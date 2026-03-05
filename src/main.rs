use bittseeder::config;
use bittseeder::seeder;
use bittseeder::stats;
use bittseeder::torrent;
use bittseeder::tracker;
use bittseeder::web;


use clap::{
    Parser,
    Subcommand
};
use config::enums::seed_protocol::SeedProtocol;
use config::structs::proxy_config::ProxyConfig;
use config::structs::seeder_config::SeederConfig;
use config::structs::torrents_file::TorrentsFile;
use config::structs::web_config::WebConfig;
use seeder::seeder::run_shared_listener;
use seeder::structs::seeder::Seeder;
use seeder::structs::torrent_registry::new_registry;
use stats::shared_stats::new_shared_stats;
use std::collections::VecDeque;
use std::path::{
    Path,
    PathBuf
};
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool,
    Ordering
};
use std::time::SystemTime;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use torrent::enums::torrent_version::TorrentVersion;
use torrent::structs::torrent_builder::TorrentBuilder;

#[derive(Subcommand, Debug)]
enum SubCmd {
    #[command(name = "hash-password")]
    HashPassword {
        password: Option<String>,
    },
}

struct BroadcastLog {
    tx: broadcast::Sender<String>,
    buffer: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
}

impl log::Log for BroadcastLog {
    fn enabled(&self, _: &log::Metadata) -> bool { true }
    fn log(&self, record: &log::Record) {
        let msg = record.args().to_string();
        let _ = self.tx.send(msg.clone());
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push_back(msg);
            if buf.len() > 10_000 {
                buf.pop_front();
            }
        }
    }
    fn flush(&self) {}
}

#[derive(Parser, Debug)]
#[command(
    name = "BittSeeder",
    about = "Unified BT+RTC BittSeeder — seed files over BitTorrent and/or WebRTC simultaneously"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<SubCmd>,
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    #[arg(long = "tracker")]
    trackers: Vec<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long = "webseed")]
    webseeds: Vec<String>,
    #[arg(long, default_value = "6881")]
    port: u16,
    #[arg(long, default_value = "false")]
    upnp: bool,
    #[arg(long = "ice")]
    ice_servers: Vec<String>,
    #[arg(long, default_value = "5000")]
    rtc_interval: u64,
    #[arg(long, value_name = "PROTOCOL", help = "Protocol: bt, rtc, or both (default: both)")]
    protocol: Option<String>,
    #[arg(long, default_value = "v1")]
    torrent_version: String,
    #[arg(long, value_name = "FILE")]
    torrent_file: Option<PathBuf>,
    #[arg(long, value_name = "MAGNET")]
    magnet: Option<String>,
    #[arg(long, default_value = "8090")]
    web_port: u16,
    #[arg(long)]
    web_password: Option<String>,
    #[arg(long, value_name = "FILE")]
    web_cert: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    web_key: Option<PathBuf>,
    #[arg(long)]
    proxy_type: Option<String>,
    #[arg(long)]
    proxy_host: Option<String>,
    #[arg(long)]
    proxy_port: Option<u16>,
    #[arg(long)]
    proxy_user: Option<String>,
    #[arg(long)]
    proxy_pass: Option<String>,
    #[arg(long)]
    log_level: Option<String>,
    files: Vec<PathBuf>,
}

fn parse_protocol(s: Option<&str>) -> SeedProtocol {
    match s.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("bt") => SeedProtocol::Bt,
        Some("rtc") => SeedProtocol::Rtc,
        _ => SeedProtocol::Both,
    }
}

async fn spawn_web_server(
    params: web::server::WebServerParams,
) -> Option<(std::thread::JoinHandle<()>, actix_web::dev::ServerHandle)> {
    let (handle_tx, handle_rx) = std::sync::mpsc::sync_channel::<actix_web::dev::ServerHandle>(1);
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build web server runtime");
        rt.block_on(async move {
            if let Err(e) = web::server::start(params, handle_tx).await {
                log::error!("[Web] Server error: {}", e);
            }
        });
    });
    match tokio::task::spawn_blocking(move || handle_rx.recv().ok())
        .await
        .ok()
        .flatten()
    {
        Some(handle) => Some((thread, handle)),
        None => {
            let _ = tokio::task::spawn_blocking(move || { let _ = thread.join(); }).await;
            None
        }
    }
}

fn parse_log_level(s: &str) -> log::LevelFilter {
    match s.to_ascii_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn"  => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _       => log::LevelFilter::Info,
    }
}

fn read_yaml_log_level(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let file: TorrentsFile = serde_yaml::from_str(&content).ok()?;
    file.config.log_level
}

fn build_proxy_from_cli(cli: &Cli) -> Option<ProxyConfig> {
    if let (Some(proxy_type), Some(proxy_host), Some(proxy_port)) =
        (&cli.proxy_type, &cli.proxy_host, cli.proxy_port)
    {
        Some(ProxyConfig {
            proxy_type: proxy_type.clone(),
            host: proxy_host.clone(),
            port: proxy_port,
            username: cli.proxy_user.clone(),
            password: cli.proxy_pass.clone(),
        })
    } else {
        None
    }
}

fn hash_password_cmd(password: Option<String>) {
    use argon2::{
        password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
        Argon2,
    };
    let pw = match password {
        Some(p) => p,
        None => {
            let first = rpassword::prompt_password("Enter password: ")
                .expect("failed to read password");
            let second = rpassword::prompt_password("Confirm password: ")
                .expect("failed to read password");
            if first != second {
                eprintln!("Error: passwords do not match.");
                std::process::exit(1);
            }
            first
        }
    };
    if pw.is_empty() {
        eprintln!("Error: password must not be empty.");
        std::process::exit(1);
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .expect("failed to hash password")
        .to_string();
    println!("{}", hash);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Some(SubCmd::HashPassword { password }) = cli.command {
        hash_password_cmd(password);
        return;
    }
    let log_buffer: std::sync::Arc<std::sync::Mutex<VecDeque<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let (log_tx, _) = broadcast::channel::<String>(4096);
    let level_filter = {
        let s = cli.log_level.clone()
            .or_else(|| {
                let config_path = cli.config.as_deref()
                    .map(Path::new)
                    .unwrap_or_else(|| Path::new("config.yaml"));
                read_yaml_log_level(config_path)
            })
            .unwrap_or_else(|| "info".to_string());
        parse_log_level(&s)
    };
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!("[{}] {}", record.level(), message))
        })
        .level(level_filter)
        .chain(std::io::stderr())
        .chain(Box::new(BroadcastLog {
            tx: log_tx.clone(),
            buffer: std::sync::Arc::clone(&log_buffer),
        }) as Box<dyn log::Log>)
        .apply()
        .expect("failed to initialize logging");
    if let Some(yaml_path) = cli.config.clone() {
        let single_mode_used = !cli.files.is_empty()
            || cli.name.is_some()
            || cli.out.is_some()
            || !cli.webseeds.is_empty()
            || !cli.ice_servers.is_empty()
            || cli.torrent_file.is_some()
            || cli.magnet.is_some();
        if single_mode_used {
            eprintln!(
                "Error: --config cannot be combined with single-torrent options \
                 (positional files, --name, --out, --webseed, --ice, --torrent-file, --magnet)."
            );
            std::process::exit(1);
        }
        let cli_proxy = build_proxy_from_cli(&cli);
        let cli_web = WebConfig {
            port: cli.web_port,
            password: cli.web_password.clone(),
            cert_path: cli.web_cert.clone(),
            key_path: cli.web_key.clone(),
        };
        run_torrents_mode(yaml_path, cli_proxy, cli_web, cli.upnp, cli.protocol.as_deref(), log_tx.clone(), std::sync::Arc::clone(&log_buffer)).await;
    } else {
        let has_input = !cli.files.is_empty() || cli.torrent_file.is_some();
        if !has_input {
            let yaml_path = PathBuf::from("config.yaml");
            let cli_proxy = build_proxy_from_cli(&cli);
            let cli_web = WebConfig {
                port: cli.web_port,
                password: cli.web_password.clone(),
                cert_path: cli.web_cert.clone(),
                key_path: cli.web_key.clone(),
            };
            run_torrents_mode(yaml_path, cli_proxy, cli_web, cli.upnp, cli.protocol.as_deref(), log_tx.clone(), std::sync::Arc::clone(&log_buffer)).await;
            return;
        }
        for path in &cli.files {
            if !path.exists() {
                eprintln!("File not found: {}", path.display());
                std::process::exit(1);
            }
        }
        if let Some(tf) = &cli.torrent_file
            && !tf.exists()
        {
            eprintln!("Torrent file not found: {}", tf.display());
            std::process::exit(1);
        }
        let version = match cli.torrent_version.as_str() {
            "v2" => TorrentVersion::V2,
            "hybrid" => TorrentVersion::Hybrid,
            _ => TorrentVersion::V1,
        };
        let proxy = build_proxy_from_cli(&cli);
        let ice_servers = if cli.ice_servers.is_empty() {
            vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
            ]
        } else {
            cli.ice_servers.clone()
        };
        let protocol = parse_protocol(cli.protocol.as_deref());
        let config = SeederConfig {
            tracker_urls: cli.trackers,
            file_paths: cli.files,
            name: cli.name,
            out_file: cli.out,
            webseed_urls: cli.webseeds,
            listen_port: cli.port,
            upnp: cli.upnp,
            ice_servers,
            rtc_interval_ms: cli.rtc_interval,
            protocol: protocol.clone(),
            version,
            torrent_file: cli.torrent_file,
            magnet: cli.magnet,
            upload_limit: None,
            proxy,
            show_stats: true,
        };
        println!("=== Seeder (BT+RTC) ===");
        println!("Protocol: {}", match &protocol {
            SeedProtocol::Bt => "bt (BitTorrent only)",
            SeedProtocol::Rtc => "rtc (WebRTC only)",
            SeedProtocol::Both => "both (BT + RTC)",
        });
        if config.tracker_urls.is_empty() && config.torrent_file.is_none() && config.magnet.is_none() {
            println!("Trackers: (none — seeding without announcing)");
        } else if !config.tracker_urls.is_empty() {
            println!("Trackers: {}", config.tracker_urls.join(", "));
        }
        if let Some(tf) = &config.torrent_file {
            println!("Torrent : {}", tf.display());
        }
        if let Some(mag) = &config.magnet {
            println!("Magnet  : {}…", &mag[..mag.len().min(60)]);
        }
        let file_list: Vec<String> = config
            .file_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        if !file_list.is_empty() {
            println!("Files   : {}", file_list.join(", "));
        }
        if config.protocol.has_bt() {
            println!("Port    : {}", config.listen_port);
        }
        if !config.webseed_urls.is_empty() {
            println!("Webseeds: {}", config.webseed_urls.join(", "));
        }
        println!();
        print!("Creating torrent (hashing pieces)… ");
        let torrent_info = match TorrentBuilder::build(&config) {
            Ok(ti) => {
                println!("done.");
                ti
            }
            Err(e) => {
                eprintln!("\nFailed to create torrent: {}", e);
                std::process::exit(1);
            }
        };
        {
            let yaml_path = PathBuf::from("torrents.yaml");
            let shared_file = Arc::new(RwLock::new(TorrentsFile::default()));
            let shared_stats = new_shared_stats();
            let (reload_tx, _reload_rx) = tokio::sync::watch::channel(());
            let web_cfg = WebConfig {
                port: cli.web_port,
                password: cli.web_password.clone(),
                cert_path: cli.web_cert.clone(),
                key_path: cli.web_key.clone(),
            };
            spawn_web_server(web::server::WebServerParams {
                config: web_cfg,
                web_threads: None,
                yaml_path,
                shared_file,
                stats: shared_stats,
                reload_tx,
                log_tx: log_tx.clone(),
                log_buffer: std::sync::Arc::clone(&log_buffer),
            }).await;
        }
        let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let mut s = Seeder::new(config, torrent_info);
        if let Err(e) = s.run(None, stop_rx).await {
            eprintln!("Fatal: {}", e);
            std::process::exit(1);
        }
    }
}

fn load_yaml(path: &Path) -> Result<TorrentsFile, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let file: TorrentsFile = serde_yaml::from_str(&content)?;
    Ok(file)
}

#[allow(clippy::type_complexity)]
fn load_yaml_entries(
    path: &Path,
    proxy: Option<&ProxyConfig>,
    upnp: bool,
    cli_protocol: Option<&str>,
) -> Result<(TorrentsFile, Vec<(String, SeederConfig)>), Box<dyn std::error::Error>> {
    let file = load_yaml(path)?;
    let effective_proxy = proxy.or(file.config.proxy.as_ref());
    let effective_upnp = upnp || file.config.upnp.unwrap_or(false);
    let effective_show_stats = file.config.show_stats.unwrap_or(true);
    let effective_listen_port = file.config.listen_port.unwrap_or(6881);
    let effective_protocol = parse_protocol(
        cli_protocol.or(file.config.protocol.as_ref().map(|p| match p {
            SeedProtocol::Bt => "bt",
            SeedProtocol::Rtc => "rtc",
            SeedProtocol::Both => "both",
        }))
    );
    let effective_ice: Vec<String> = file.config.rtc_ice_servers.clone().unwrap_or_else(|| vec![
        "stun:stun.l.google.com:19302".to_string(),
        "stun:stun1.l.google.com:19302".to_string(),
    ]);
    let effective_rtc_interval_ms = file.config.rtc_interval_ms.unwrap_or(5000);
    let mut result = Vec::new();
    for (i, entry) in file.torrents.iter().enumerate() {
        if !entry.enabled {
            let label = entry.name.clone().unwrap_or_else(|| format!("torrent-{}", i));
            println!("[{}] disabled — skipping", label);
            continue;
        }
        match entry.to_seeder_config(
            effective_proxy,
            effective_listen_port,
            effective_protocol.clone(),
            &effective_ice,
            effective_rtc_interval_ms,
        ) {
            Ok(mut cfg) => {
                cfg.upnp = effective_upnp;
                cfg.show_stats = effective_show_stats;
                let label = cfg
                    .name
                    .clone()
                    .or_else(|| cfg.file_paths.first().map(|p| p.display().to_string()))
                    .or_else(|| cfg.torrent_file.as_ref().map(|p| p.display().to_string()))
                    .unwrap_or_else(|| format!("torrent-{}", i));
                result.push((label, cfg));
            }
            Err(e) => {
                eprintln!("[BittSeeder] Skipping entry {}: {}", i, e);
            }
        }
    }
    Ok((file, result))
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

async fn seed_one(
    label: String,
    config: SeederConfig,
    registry: Option<seeder::structs::torrent_registry::TorrentRegistry>,
    stop_rx: tokio::sync::watch::Receiver<bool>,
    shared_stats: stats::shared_stats::SharedStats,
) {
    if config.tracker_urls.is_empty() && config.torrent_file.is_none() && config.magnet.is_none() {
        println!("[{}] Trackers: (none)", label);
    } else if !config.tracker_urls.is_empty() {
        println!("[{}] Trackers: {}", label, config.tracker_urls.join(", "));
    }
    if let Some(tf) = &config.torrent_file {
        println!("[{}] Torrent : {}", label, tf.display());
    }
    let files: Vec<String> = config.file_paths.iter().map(|p| p.display().to_string()).collect();
    if !files.is_empty() {
        println!("[{}] Files   : {}", label, files.join(", "));
    }
    if !config.webseed_urls.is_empty() {
        println!("[{}] Webseeds: {}", label, config.webseed_urls.join(", "));
    }
    let proto_str = match &config.protocol {
        SeedProtocol::Bt => "bt",
        SeedProtocol::Rtc => "rtc",
        SeedProtocol::Both => "both",
    };
    let version_str = match config.version {
        TorrentVersion::V1 => "v1",
        TorrentVersion::V2 => "v2",
        TorrentVersion::Hybrid => "hybrid",
    };
    print!("[{}] Hashing pieces ({}, protocol={})… ", label, version_str, proto_str);
    let torrent_info = match TorrentBuilder::build(&config) {
        Ok(ti) => {
            println!("done.");
            ti
        }
        Err(e) => {
            eprintln!("\n[{}] Failed to create torrent: {}", label, e);
            return;
        }
    };
    let mut s = Seeder::new(config, torrent_info);
    {
        use std::sync::atomic::Ordering;
        let uploaded_arc  = Arc::clone(&s.uploaded);
        let peer_count_arc = Arc::clone(&s.peer_count);
        let peers_arc     = Arc::clone(&s.peers);
        let stats_map     = Arc::clone(&shared_stats);
        let stats_label   = label.clone();
        let mut srx       = stop_rx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        let bt   = peer_count_arc.load(Ordering::Relaxed);
                        let rtc  = peers_arc.lock().await.len();
                        let up   = uploaded_arc.load(Ordering::Relaxed);
                        stats_map.write().await.insert(
                            stats_label.clone(),
                            stats::shared_stats::TorrentStats { uploaded: up, peer_count: bt + rtc },
                        );
                    }
                    _ = srx.changed() => {
                        if *srx.borrow() {
                            stats_map.write().await.remove(&stats_label);
                            break;
                        }
                    }
                }
            }
        });
    }

    if let Err(e) = s.run(registry, stop_rx).await {
        eprintln!("[{}] Fatal: {}", label, e);
    }
    shared_stats.write().await.remove(&label);
}

fn build_seeder_runtime(seeder_threads: Option<usize>) -> tokio::runtime::Runtime {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    if let Some(n) = seeder_threads {
        builder.worker_threads(n);
    }
    builder.enable_all().build().expect("failed to build seeder runtime")
}

async fn run_torrents_mode(
    yaml_path: PathBuf,
    cli_proxy: Option<ProxyConfig>,
    cli_web: WebConfig,
    cli_upnp: bool,
    cli_protocol: Option<&str>,
    log_tx: broadcast::Sender<String>,
    log_buffer: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
) {
    println!("=== Seeder (BT+RTC, multi-torrent mode) ===");
    println!("Config  : {}", yaml_path.display());
    println!();
    if !yaml_path.exists() {
        println!("[BittSeeder] Creating empty config file: {}", yaml_path.display());
        let empty = TorrentsFile::default();
        let s = serde_yaml::to_string(&empty).expect("serialize empty TorrentsFile");
        std::fs::write(&yaml_path, s).expect("write empty YAML");
    }
    #[cfg(unix)]
    let mut sighup = {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::hangup()).expect("failed to install SIGHUP handler")
    };
    let shared_file: Arc<RwLock<TorrentsFile>> = Arc::new(RwLock::new(TorrentsFile::default()));
    let shared_stats = new_shared_stats();
    let (reload_tx, mut reload_rx) = tokio::sync::watch::channel(());
    let yaml_for_web = match load_yaml(&yaml_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[BittSeeder] Failed to load {}: {}", yaml_path.display(), e);
            std::process::exit(1);
        }
    };
    let cli_web_port = cli_web.port;
    let cli_web_password = cli_web.password.clone();
    let cli_web_cert     = cli_web.cert_path.clone();
    let cli_web_key      = cli_web.key_path.clone();
    let mut current_web_threads = yaml_for_web.config.web_threads;
    let mut current_web_cert: Option<PathBuf> = cli_web_cert.clone().or_else(|| yaml_for_web.config.web_cert.clone());
    let mut current_web_key:  Option<PathBuf> = cli_web_key.clone().or_else(|| yaml_for_web.config.web_key.clone());
    let initial_web_cfg = WebConfig {
        port: cli_web_port,
        password: cli_web_password.clone().or_else(|| yaml_for_web.config.web_password.clone()),
        cert_path: current_web_cert.clone(),
        key_path:  current_web_key.clone(),
    };
    let force_web_restart = Arc::new(AtomicBool::new(false));
    let mut web_server_info = spawn_web_server(web::server::WebServerParams {
        config: initial_web_cfg,
        web_threads: current_web_threads,
        yaml_path: yaml_path.clone(),
        shared_file: Arc::clone(&shared_file),
        stats: Arc::clone(&shared_stats),
        reload_tx: reload_tx.clone(),
        log_tx: log_tx.clone(),
        log_buffer: std::sync::Arc::clone(&log_buffer),
    }).await;
    {
        let shared_file_acme = Arc::clone(&shared_file);
        let reload_tx_acme = reload_tx.clone();
        let yaml_path_acme = yaml_path.clone();
        let force_restart = Arc::clone(&force_web_restart);
        tokio::spawn(async move {
            use tokio::time::{interval, Duration, MissedTickBehavior};
            let mut ticker = interval(Duration::from_secs(12 * 3600));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let cfg = shared_file_acme.read().await.config.clone();
                if let (Some(domain), Some(email)) = (cfg.letsencrypt_domain, cfg.letsencrypt_email) {
                    let http_port = cfg.letsencrypt_http_port.unwrap_or(80);
                    let yaml_dir = yaml_path_acme.parent()
                        .unwrap_or(std::path::Path::new("."))
                        .to_path_buf();
                    let cert_path = yaml_dir.join("bittseeder.crt");
                    let key_path = yaml_dir.join("bittseeder.key");
                    let acct_path = yaml_dir.join("bittseeder-account.key");
                    match web::acme::ensure_certificate(
                        &domain, &email, http_port, &cert_path, &key_path, &acct_path,
                    ).await {
                        Ok(true) => {
                            log::info!("[ACME] Certificate renewed for {} — restarting web server…", domain);
                            {
                                let mut f = shared_file_acme.write().await;
                                f.config.web_cert = Some(cert_path.clone());
                                f.config.web_key = Some(key_path.clone());
                                let s = serde_yaml::to_string(&*f).ok();
                                drop(f);
                                if let Some(s) = s {
                                    std::fs::write(&yaml_path_acme, s).ok();
                                }
                            }
                            force_restart.store(true, Ordering::Release);
                            let _ = reload_tx_acme.send(());
                        }
                        Ok(false) => log::debug!("[ACME] Certificate still valid, no renewal needed"),
                        Err(e) => log::error!("[ACME] Certificate renewal failed: {}", e),
                    }
                }
            }
        });
    }
    loop {
        let (file, entries) = match load_yaml_entries(&yaml_path, cli_proxy.as_ref(), cli_upnp, cli_protocol) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[BittSeeder] Failed to load {}: {}", yaml_path.display(), e);
                std::process::exit(1);
            }
        };
        let effective_listen_port = file.config.listen_port.unwrap_or(6881);
        let effective_upnp = cli_upnp || file.config.upnp.unwrap_or(false);
        let effective_protocol = parse_protocol(
            cli_protocol.or(file.config.protocol.as_ref().map(|p| match p {
                SeedProtocol::Bt => "bt",
                SeedProtocol::Rtc => "rtc",
                SeedProtocol::Both => "both",
            }))
        );
        let new_seeder_threads = file.config.seeder_threads;
        let new_web_threads = file.config.web_threads;
        let new_web_cert: Option<PathBuf> = cli_web_cert.clone().or_else(|| file.config.web_cert.clone());
        let new_web_key:  Option<PathBuf> = cli_web_key.clone().or_else(|| file.config.web_key.clone());
        {
            let mut sf = shared_file.write().await;
            *sf = file;
        }
        let force_restart = force_web_restart.swap(false, Ordering::AcqRel);
        if new_web_threads != current_web_threads
            || new_web_cert != current_web_cert
            || new_web_key != current_web_key
            || force_restart
        {
            let reason = if new_web_threads != current_web_threads {
                format!(
                    "thread count ({} → {})",
                    current_web_threads.map(|n| n.to_string()).unwrap_or_else(|| "auto".to_string()),
                    new_web_threads.map(|n| n.to_string()).unwrap_or_else(|| "auto".to_string()),
                )
            } else if force_restart || new_web_cert != current_web_cert || new_web_key != current_web_key {
                "TLS certificate/key changed".to_string()
            } else {
                "configuration changed".to_string()
            };
            log::info!("[Web] {} — restarting web server…", reason);
            if let Some((old_thread, old_handle)) = web_server_info.take() {
                old_handle.stop(true).await;
                tokio::task::spawn_blocking(move || { let _ = old_thread.join(); }).await.ok();
            }
            let cfg_now = shared_file.read().await.config.clone();
            let new_web_cfg = WebConfig {
                port: cli_web_port,
                password: cli_web_password.clone().or_else(|| cfg_now.web_password.clone()),
                cert_path: new_web_cert.clone(),
                key_path:  new_web_key.clone(),
            };
            web_server_info = spawn_web_server(web::server::WebServerParams {
                config: new_web_cfg,
                web_threads: new_web_threads,
                yaml_path: yaml_path.clone(),
                shared_file: Arc::clone(&shared_file),
                stats: Arc::clone(&shared_stats),
                reload_tx: reload_tx.clone(),
                log_tx: log_tx.clone(),
                log_buffer: std::sync::Arc::clone(&log_buffer),
            }).await;
            current_web_threads = new_web_threads;
            current_web_cert = new_web_cert;
            current_web_key = new_web_key;
        }
        let seeder_rt = build_seeder_runtime(new_seeder_threads);
        {
            let label = new_seeder_threads
                .map(|n| format!("{} thread(s)", n))
                .unwrap_or_else(|| "auto (CPU count)".to_string());
            log::info!("[BittSeeder] Seeder runtime: {}", label);
        }
        if entries.is_empty() {
            println!("[BittSeeder] No enabled torrent entries — waiting for changes…");
        } else {
            println!("[BittSeeder] Starting {} torrent(s)…", entries.len());
        }
        let registry = new_registry();
        let listener_handle = if effective_protocol.has_bt() {
            let reg = Arc::clone(&registry);
            Some(seeder_rt.spawn(async move {
                run_shared_listener(effective_listen_port, reg, effective_upnp).await;
            }))
        } else {
            None
        };
        let mut stop_txs: Vec<tokio::sync::watch::Sender<bool>> = Vec::new();
        let handles: Vec<_> = entries
            .into_iter()
            .map(|(label, cfg)| {
                let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
                stop_txs.push(stop_tx);
                let reg = if cfg.protocol.has_bt() { Some(Arc::clone(&registry)) } else { None };
                let ss = Arc::clone(&shared_stats);
                seeder_rt.spawn(seed_one(label, cfg, reg, stop_rx, ss))
            })
            .collect();
        let initial_mtime = file_mtime(&yaml_path);
        let should_reload = 'wait: loop {
            #[cfg(unix)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break 'wait false,
                _ = sighup.recv() => {
                    println!("[BittSeeder] SIGHUP received — reloading…");
                    break 'wait true;
                },
                _ = reload_rx.changed() => {
                    println!("[BittSeeder] Web UI triggered reload…");
                    break 'wait true;
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    if file_mtime(&yaml_path) != initial_mtime {
                        println!("[BittSeeder] Config file changed on disk — reloading…");
                        break 'wait true;
                    }
                }
            }
            #[cfg(not(unix))]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break 'wait false,
                _ = reload_rx.changed() => {
                    println!("[BittSeeder] Web UI triggered reload…");
                    break 'wait true;
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    if file_mtime(&yaml_path) != initial_mtime {
                        println!("[BittSeeder] Config file changed on disk — reloading…");
                        break 'wait true;
                    }
                }
            }
        };
        if let Some(h) = listener_handle {
            h.abort();
            let _ = h.await;
        }
        for tx in &stop_txs {
            let _ = tx.send(true);
        }
        let shutdown_msg = if should_reload {
            "[BittSeeder] Reloading — waiting for seeders to send 'stopped' announces (up to 15s)…"
        } else {
            "[BittSeeder] Shutting down — waiting for seeders to send 'stopped' announces (up to 15s)…"
        };
        println!("{}", shutdown_msg);
        let abort_handles: Vec<_> = handles.iter().map(|h| h.abort_handle()).collect();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            async {
                for h in handles {
                    let _ = h.await;
                }
            },
        ).await;
        for ah in abort_handles {
            ah.abort();
        }
        tokio::task::spawn_blocking(move || {
            seeder_rt.shutdown_timeout(std::time::Duration::from_secs(5));
        }).await.ok();
        if !should_reload {
            if let Some((_, handle)) = web_server_info.take() {
                handle.stop(true).await;
            }
            println!("[BittSeeder] Shutting down.");
            break;
        }
        println!("[BittSeeder] Applying new config…\n");
    }
}