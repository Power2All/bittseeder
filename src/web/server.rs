use crate::config::structs::torrents_file::TorrentsFile;
use crate::config::structs::web_config::WebConfig;
use crate::stats::shared_stats::SharedStats;
use crate::web::api::{
    add_torrent,
    batch_add,
    browse,
    delete_torrent,
    delete_2fa_disable,
    file_upload_cancel,
    file_upload_hash_progress,
    file_upload_chunk,
    file_upload_finalize,
    file_upload_init,
    get_auth_info,
    get_config,
    get_index,
    get_logo,
    get_status,
    get_torrents,
    get_ws,
    mkdir,
    post_login,
    post_logout,
    post_2fa_confirm,
    post_2fa_setup,
    update_config,
    update_torrent,
    upload_torrent,
};
use crate::web::structs::app_state::{
    AppState,
    SessionStore,
    UploadStore,
};
use actix_web::{
    web::{
        self,
        Data
    },
    App,
    HttpServer,
};
use std::collections::{
    HashMap,
    VecDeque
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{
    broadcast,
    watch,
    Mutex,
    RwLock
};

pub struct WebServerParams {
    pub config: WebConfig,
    pub web_threads: Option<usize>,
    pub yaml_path: PathBuf,
    pub shared_file: Arc<RwLock<TorrentsFile>>,
    pub stats: SharedStats,
    pub reload_tx: watch::Sender<()>,
    pub log_tx: broadcast::Sender<String>,
    pub log_buffer: Arc<std::sync::Mutex<VecDeque<String>>>,
}

pub async fn start(
    params: WebServerParams,
    server_handle_tx: std::sync::mpsc::SyncSender<actix_web::dev::ServerHandle>,
) -> std::io::Result<()> {
    let WebServerParams {
        config,
        web_threads,
        yaml_path,
        shared_file,
        stats,
        reload_tx,
        log_tx,
        log_buffer,
    } = params;
    let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
    let uploads: UploadStore  = Arc::new(Mutex::new(HashMap::new()));
    let login_attempts = Arc::new(Mutex::new(HashMap::new()));
    let state = Data::new(AppState {
        yaml_path,
        shared_file,
        stats,
        reload_tx,
        web_password: config.password.clone(),
        sessions,
        log_tx,
        log_buffer,
        uploads,
        login_attempts,
    });
    let cert_key = if let (Some(cert), Some(key)) = (config.cert_path, config.key_path) {
        Some((cert, key))
    } else {
        None
    };
    let bind_addr = format!("0.0.0.0:{}", config.port);
    let thread_label = web_threads
        .map(|n| format!("{}", n))
        .unwrap_or_else(|| "auto".to_string());
    log::info!(
        "[Web] Starting on http{}://{} ({} worker thread(s))",
        if cert_key.is_some() { "s" } else { "" },
        bind_addr,
        thread_label
    );
    let mut server_builder = HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::JsonConfig::default().limit(1024 * 1024))
            .app_data(web::PayloadConfig::default().limit(32 * 1024 * 1024))
            .route("/", web::get().to(get_index))
            .route("/logo.png", web::get().to(get_logo))
            .route("/api/ws", web::get().to(get_ws))
            .route("/api/auth-info", web::get().to(get_auth_info))
            .route("/api/login", web::post().to(post_login))
            .route("/api/logout", web::post().to(post_logout))
            .route("/api/2fa/setup", web::post().to(post_2fa_setup))
            .route("/api/2fa/confirm", web::post().to(post_2fa_confirm))
            .route("/api/2fa/disable", web::delete().to(delete_2fa_disable))
            .route("/api/status", web::get().to(get_status))
            .route("/api/config", web::get().to(get_config))
            .route("/api/config", web::put().to(update_config))
            .route("/api/torrents", web::get().to(get_torrents))
            .route("/api/torrents", web::post().to(add_torrent))
            .route("/api/torrents/{idx}", web::put().to(update_torrent))
            .route("/api/torrents/{idx}", web::delete().to(delete_torrent))
            .route("/api/browse", web::get().to(browse))
            .route("/api/upload-torrent", web::post().to(upload_torrent))
            .route("/api/batch-add", web::post().to(batch_add))
            .route("/api/mkdir", web::post().to(mkdir))
            .route("/api/file-upload/init", web::post().to(file_upload_init))
            .route("/api/file-upload/chunk", web::post().to(file_upload_chunk))
            .route("/api/file-upload/finalize", web::post().to(file_upload_finalize))
            .route("/api/file-upload/{upload_id}", web::delete().to(file_upload_cancel))
            .route("/api/file-upload/{upload_id}/hash-progress", web::get().to(file_upload_hash_progress))
    });
    if let Some(n) = web_threads {
        server_builder = server_builder.workers(n);
    }
    let running = if let Some((cert_path, key_path)) = cert_key {
        let cert_data = std::fs::read(&cert_path)?;
        let key_data = std::fs::read(&key_path)?;
        let mut cert_reader = std::io::BufReader::new(cert_data.as_slice());
        let mut key_reader = std::io::BufReader::new(key_data.as_slice());
        let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut cert_reader)
                .filter_map(|c| c.ok())
                .map(|c| c.into_owned())
                .collect();
        let key = rustls_pemfile::private_key(&mut key_reader)
            .ok()
            .flatten()
            .ok_or_else(|| std::io::Error::other("no private key found"))?
            .clone_key();
        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(std::io::Error::other)?;
        server_builder.bind_rustls_0_23(&bind_addr, tls_config)?.run()
    } else {
        server_builder.bind(&bind_addr)?.run()
    };
    let _ = server_handle_tx.send(running.handle());
    running.await
}