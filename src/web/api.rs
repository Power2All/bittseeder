use crate::config::structs::global_config::GlobalConfig;
use crate::config::structs::torrent_entry::TorrentEntry;
use crate::config::structs::torrents_file::TorrentsFile;
use crate::stats::shared_stats::SharedStats;
use crate::web::structs::app_state::AppState;
use crate::web::structs::upload_session::UploadSession;
use actix_web::{
    web::{
        Bytes,
        Data,
        Json,
        Path,
        Payload,
        Query,
    },
    HttpRequest,
    HttpResponse,
};
use sha2::{
    Digest,
    Sha256,
};
use argon2::{
    Argon2,
    PasswordHash,
    PasswordVerifier
};
use futures_util::StreamExt as _;
use serde::Deserialize;
use serde_json::json;
use std::collections::{
    HashSet,
    VecDeque,
};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{
    Duration,
    Instant
};
use tokio::sync::broadcast;

const SESSION_TTL: Duration = Duration::from_secs(3600);

pub fn verify_totp(secret_base32: &str, code: &str) -> bool {
    use totp_rs::{Algorithm, Secret, TOTP};
    let Ok(bytes) = Secret::Encoded(secret_base32.to_string()).to_bytes() else { return false };
    let Ok(totp) = TOTP::new(
        Algorithm::SHA1, 6, 1, 30, bytes,
        Some("BittSeeder".to_string()), "admin".to_string(),
    ) else { return false };
    totp.check_current(code).unwrap_or(false)
}

pub fn verify_password(input: &str, stored: &str) -> bool {
    if stored.starts_with("$argon2") {
        match PasswordHash::new(stored) {
            Ok(parsed) => Argon2::default()
                .verify_password(input.as_bytes(), &parsed)
                .is_ok(),
            Err(_) => false,
        }
    } else {
        input == stored
    }
}

fn extract_token(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

pub async fn is_authenticated(req: &HttpRequest, data: &Data<AppState>) -> bool {
    if data.web_password.is_none() {
        return true;
    }
    let token = match extract_token(req) {
        Some(t) => t,
        None => return false,
    };
    let mut sessions = data.sessions.lock().await;
    if let Some(expiry) = sessions.get(&token) {
        if Instant::now() < *expiry {
            sessions.insert(token, Instant::now() + SESSION_TTL);
            return true;
        }
        sessions.remove(&token);
    }
    false
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
    pub totp_code: Option<String>,
}

pub async fn post_login(req: HttpRequest, data: Data<AppState>, body: Json<LoginRequest>) -> HttpResponse {
    let peer_ip = req
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let rate_limit = {
        let file = data.shared_file.read().await;
        file.config.web_login_rate_limit.unwrap_or(10)
    };
    if rate_limit > 0 {
        let mut attempts = data.login_attempts.lock().await;
        let now = std::time::Instant::now();
        let entry = attempts.entry(peer_ip).or_insert((0, now));
        if now.duration_since(entry.1) >= std::time::Duration::from_secs(60) {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > rate_limit {
                return HttpResponse::TooManyRequests()
                    .json(json!({"error": "Too many login attempts. Please try again later."}));
            }
        }
    }
    if let Some(ref expected) = data.web_password {
        if !verify_password(&body.password, expected) {
            return HttpResponse::Unauthorized().json(json!({"error": "Invalid password"}));
        }
    }
    let totp_secret = {
        let file = data.shared_file.read().await;
        file.config.totp_secret.clone()
    };
    if let Some(ref secret) = totp_secret {
        let code = body.totp_code.as_deref().unwrap_or("").trim();
        if code.is_empty() {
            return HttpResponse::Unauthorized()
                .json(json!({"error": "TOTP code required", "requires_totp": true}));
        }
        if !verify_totp(secret, code) {
            return HttpResponse::Unauthorized().json(json!({"error": "Invalid TOTP code"}));
        }
    }
    if data.web_password.is_none() {
        HttpResponse::Ok().json(json!({"token": "noauth"}))
    } else {
        let token = generate_token();
        let expiry = Instant::now() + SESSION_TTL;
        data.sessions.lock().await.insert(token.clone(), expiry);
        HttpResponse::Ok().json(json!({"token": token}))
    }
}

pub async fn get_auth_info(data: Data<AppState>) -> HttpResponse {
    let file = data.shared_file.read().await;
    let requires_totp = file.config.totp_secret.is_some();
    drop(file);
    HttpResponse::Ok().json(json!({
        "requires_password": data.web_password.is_some(),
        "requires_totp":     requires_totp,
    }))
}

pub async fn post_2fa_setup(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    use totp_rs::{Algorithm, Secret, TOTP};
    let secret = Secret::generate_secret().to_encoded();
    let bytes = match secret.to_bytes() {
        Ok(b) => b,
        Err(_) => return HttpResponse::InternalServerError()
            .json(json!({"error": "Failed to generate secret"})),
    };
    let totp = match TOTP::new(
        Algorithm::SHA1, 6, 1, 30, bytes,
        Some("BittSeeder".to_string()), "admin".to_string(),
    ) {
        Ok(t) => t,
        Err(_) => return HttpResponse::InternalServerError()
            .json(json!({"error": "Failed to create TOTP"})),
    };
    HttpResponse::Ok().json(json!({
        "secret":      secret.to_string(),
        "otpauth_uri": totp.get_url(),
    }))
}

#[derive(Deserialize)]
pub struct TwoFAConfirmRequest {
    pub secret: String,
    pub code: String,
}

pub async fn post_2fa_confirm(
    req: HttpRequest,
    data: Data<AppState>,
    body: Json<TwoFAConfirmRequest>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    if !verify_totp(&body.secret, &body.code) {
        return HttpResponse::Unauthorized().json(json!({"error": "Invalid TOTP code"}));
    }
    let mut file = data.shared_file.write().await;
    file.config.totp_secret = Some(body.secret.clone());
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    log::info!("[Web] 2FA enabled");
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn delete_2fa_disable(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut file = data.shared_file.write().await;
    file.config.totp_secret = None;
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    log::info!("[Web] 2FA disabled");
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn post_logout(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if let Some(token) = extract_token(&req) {
        data.sessions.lock().await.remove(&token);
    }
    HttpResponse::Ok().json(json!({"ok": true}))
}

fn generate_token() -> String {
    use rand::RngExt;
    let bytes: [u8; 24] = rand::rng().random();
    hex::encode(bytes)
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

pub async fn browse(req: HttpRequest, query: Query<BrowseQuery>, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let raw = query.path.as_deref().unwrap_or("");
    let dir_buf;
    let dir = if raw.is_empty() {
        dir_buf = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("/"));
        dir_buf.as_path()
    } else {
        dir_buf = std::path::PathBuf::from(raw);
        dir_buf.as_path()
    };
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return HttpResponse::BadRequest().body(e.to_string()),
    };
    let mut dir_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
    dir_entries.sort_by_key(|e| {
        let is_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        (is_file as u8, e.file_name().to_string_lossy().to_lowercase())
    });
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for entry in dir_entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        entries.push(json!({ "name": name, "is_dir": is_dir, "size": size }));
    }
    let parent = dir.parent().map(|p| p.to_string_lossy().into_owned());
    let current = dir.to_string_lossy().into_owned();
    HttpResponse::Ok().json(json!({
        "path": current,
        "parent": parent,
        "entries": entries,
    }))
}

fn write_yaml(path: &std::path::Path, file: &TorrentsFile) -> io::Result<()> {
    let s = serde_yaml::to_string(file).map_err(io::Error::other)?;
    std::fs::write(path, s)
}

pub async fn get_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(include_str!("index.html"))
}

pub async fn get_logo() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("image/png")
        .insert_header(("Cache-Control", "public, max-age=86400"))
        .body(include_bytes!("logo.png").as_ref())
}

pub async fn get_style() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/css; charset=utf-8")
        .insert_header(("Cache-Control", "public, max-age=3600"))
        .body(include_str!("style.css"))
}

pub async fn get_app_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .insert_header(("Cache-Control", "public, max-age=3600"))
        .body(include_str!("app.js"))
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

pub async fn get_ws(
    req: HttpRequest,
    stream: Payload,
    data: Data<AppState>,
    query: Query<WsQuery>,
) -> Result<HttpResponse, actix_web::Error> {
    let authenticated = if data.web_password.is_none() {
        true
    } else {
        let token = query.token.as_deref().unwrap_or("");
        if token.is_empty() {
            false
        } else {
            let sessions = data.sessions.lock().await;
            sessions.get(token).map(|exp| Instant::now() < *exp).unwrap_or(false)
        }
    };
    if !authenticated {
        return Ok(HttpResponse::Unauthorized().finish());
    }
    let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;
    let buffered: Vec<String> = {
        let buf = data.log_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.iter().cloned().collect()
    };
    let log_rx = data.log_tx.subscribe();
    let stats = Arc::clone(&data.stats);
    actix_web::rt::spawn(async move {
        ws_loop(session, msg_stream, log_rx, stats, buffered).await;
    });
    Ok(res)
}

async fn ws_loop(
    mut session: actix_ws::Session,
    mut stream: actix_ws::MessageStream,
    mut log_rx: broadcast::Receiver<String>,
    stats: SharedStats,
    buffered_logs: Vec<String>,
) {
    for line in &buffered_logs {
        let msg = serde_json::to_string(&json!({ "type": "log", "line": line }))
            .unwrap_or_default();
        if session.text(msg).await.is_err() {
            return;
        }
    }
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut last_uploaded: u64 = 0;
    let mut last_tick = Instant::now();
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let st = stats.read().await;
                let total_peers: usize = st.values().map(|v| v.peer_count).sum();
                let total_uploaded: u64 = st.values().map(|v| v.uploaded).sum();
                let torrents_map: std::collections::HashMap<String, serde_json::Value> = st
                    .iter()
                    .map(|(k, v)| {
                        (k.clone(), json!({
                            "uploaded":   v.uploaded,
                            "peer_count": v.peer_count,
                        }))
                    })
                    .collect();
                drop(st);
                let elapsed = last_tick.elapsed().as_secs_f64().max(0.001);
                let rate = if total_uploaded >= last_uploaded {
                    ((total_uploaded - last_uploaded) as f64 / elapsed) as u64
                } else {
                    0
                };
                last_uploaded = total_uploaded;
                last_tick = Instant::now();
                let msg = serde_json::to_string(&json!({
                    "type": "stats",
                    "ts": ts,
                    "peers": total_peers,
                    "rate": rate,
                    "torrents": torrents_map,
                })).unwrap_or_default();
                if session.text(msg).await.is_err() { break; }
            }
            result = log_rx.recv() => {
                match result {
                    Ok(line) => {
                        let msg = serde_json::to_string(&json!({ "type": "log", "line": line }))
                            .unwrap_or_default();
                        if session.text(msg).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(actix_ws::Message::Close(reason))) => {
                        let _ = session.close(reason).await;
                        return;
                    }
                    Some(Ok(actix_ws::Message::Ping(bytes))) => {
                        if session.pong(&bytes).await.is_err() { break; }
                    }
                    None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = session.close(None).await;
}

const _: fn() = || {
    let _: VecDeque<String>;
};

pub async fn get_status(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let stats = data.stats.read().await;
    let map: std::collections::HashMap<String, serde_json::Value> = stats
        .iter()
        .map(|(k, v): (&String, &crate::stats::shared_stats::TorrentStats)| {
            (
                k.clone(),
                json!({
                    "uploaded": v.uploaded,
                    "peer_count": v.peer_count,
                }),
            )
        })
        .collect();
    HttpResponse::Ok().json(json!({ "torrents": map }))
}

pub async fn get_torrents(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let file = data.shared_file.read().await;
    HttpResponse::Ok().json(&file.torrents)
}

pub async fn add_torrent(req: HttpRequest, data: Data<AppState>, body: Json<TorrentEntry>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut file = data.shared_file.write().await;
    file.torrents.push(body.into_inner());
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    let _ = data.reload_tx.send(());
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn update_torrent(
    req: HttpRequest,
    data: Data<AppState>,
    idx: Path<usize>,
    body: Json<TorrentEntry>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut file = data.shared_file.write().await;
    let i = idx.into_inner();
    if i >= file.torrents.len() {
        return HttpResponse::NotFound().body("index out of range");
    }
    file.torrents[i] = body.into_inner();
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    let _ = data.reload_tx.send(());
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn delete_torrent(req: HttpRequest, data: Data<AppState>, idx: Path<usize>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut file = data.shared_file.write().await;
    let i = idx.into_inner();
    if i >= file.torrents.len() {
        return HttpResponse::NotFound().body("index out of range");
    }
    file.torrents.remove(i);
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    let _ = data.reload_tx.send(());
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn get_config(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let file = data.shared_file.read().await;
    let totp_enabled = file.config.totp_secret.is_some();
    let mut config_json = match serde_json::to_value(&file.config) {
        Ok(v) => v,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    drop(file);
    if let serde_json::Value::Object(ref mut map) = config_json {
        map.remove("web_password");
        map.remove("totp_secret");
        map.insert("totp_enabled".to_string(), serde_json::Value::Bool(totp_enabled));
    }
    let cert_path = data.yaml_path.parent()
        .unwrap_or(std::path::Path::new("."))
        .join("bittseeder.crt");
    if cert_path.exists()
        && let Ok(meta) = std::fs::metadata(&cert_path)
        && let Ok(modified) = meta.modified()
    {
        use chrono::{DateTime, Utc};
        let expiry_time = modified + std::time::Duration::from_secs(90 * 24 * 3600);
        let dt: DateTime<Utc> = DateTime::from(expiry_time);
        let expiry_str = dt.format("%Y-%m-%d").to_string();
        if let serde_json::Value::Object(ref mut map) = config_json {
            map.insert("le_cert_expiry".to_string(), serde_json::Value::String(expiry_str));
        }
    }
    HttpResponse::Ok().json(config_json)
}

pub async fn update_config(req: HttpRequest, data: Data<AppState>, body: Json<GlobalConfig>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut new_cfg = body.into_inner();
    if let Some(ref level_str) = new_cfg.log_level {
        let filter = match level_str.to_ascii_lowercase().as_str() {
            "error" => log::LevelFilter::Error,
            "warn" => log::LevelFilter::Warn,
            "debug" => log::LevelFilter::Debug,
            "trace" => log::LevelFilter::Trace,
            _ => log::LevelFilter::Info,
        };
        log::set_max_level(filter);
        log::info!("[Config] Log level set to {}", level_str);
    }
    let mut file = data.shared_file.write().await;
    if new_cfg.web_password.is_none() {
        new_cfg.web_password = file.config.web_password.clone();
    }
    if new_cfg.totp_secret.is_none() {
        new_cfg.totp_secret = file.config.totp_secret.clone();
    }
    let password_changed = new_cfg.web_password != file.config.web_password;
    file.config = new_cfg;
    if let Err(e) = write_yaml(&data.yaml_path, &file) {
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    drop(file);
    if password_changed {
        data.sessions.lock().await.clear();
        log::info!("[Config] Password changed — all active sessions invalidated");
    }
    let _ = data.reload_tx.send(());
    HttpResponse::Ok().json(json!({"ok": true}))
}

#[derive(Deserialize)]
pub struct UploadTorrentQuery {
    pub name: Option<String>,
}

pub async fn upload_torrent(
    req: HttpRequest,
    data: Data<AppState>,
    query: Query<UploadTorrentQuery>,
    body: Bytes,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    if body.is_empty() {
        return HttpResponse::BadRequest().json(json!({"error": "File body is empty"}));
    }
    let raw_name = query.name.as_deref().unwrap_or("upload.torrent");
    let base_name = std::path::Path::new(raw_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload.torrent");
    let filename = if base_name.to_ascii_lowercase().ends_with(".torrent") {
        base_name.to_string()
    } else {
        format!("{}.torrent", base_name)
    };
    let base_dir = data.yaml_path.parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let upload_dir = base_dir.join("uploaded_torrents");
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        return HttpResponse::InternalServerError()
            .body(format!("Failed to create upload directory: {}", e));
    }
    let dest = upload_dir.join(&filename);
    if let Err(e) = std::fs::write(&dest, &body) {
        return HttpResponse::InternalServerError()
            .body(format!("Failed to write torrent file: {}", e));
    }
    log::info!("[Web] Uploaded torrent: {}", dest.display());
    HttpResponse::Ok().json(json!({
        "path": dest.to_string_lossy(),
        "name": filename,
    }))
}

#[derive(Deserialize)]
pub struct MkdirRequest {
    pub path: String,
}

pub async fn mkdir(req: HttpRequest, data: Data<AppState>, body: Json<MkdirRequest>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let path = std::path::Path::new(&body.path);
    if path.components().any(|c| c == std::path::Component::ParentDir) {
        return HttpResponse::BadRequest()
            .json(json!({"error": "Path must not contain '..' components"}));
    }
    if let Err(e) = std::fs::create_dir(path) {
        return HttpResponse::BadRequest().json(json!({"error": e.to_string()}));
    }
    log::info!("[Web] Created directory: {}", path.display());
    HttpResponse::Ok().json(json!({"ok": true, "path": body.path}))
}

pub async fn batch_add(req: HttpRequest, data: Data<AppState>) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let source_folder = {
        let file = data.shared_file.read().await;
        file.config.source_folder.clone()
    };
    let source_folder = match source_folder {
        Some(p) => p,
        None => return HttpResponse::BadRequest().json(json!({
            "error": "No source_folder is configured. Set it in Global Settings → Network first."
        })),
    };
    if !source_folder.exists() {
        return HttpResponse::BadRequest().json(json!({
            "error": format!("Source folder does not exist: {}", source_folder.display()),
        }));
    }
    let read_dir = match std::fs::read_dir(&source_folder) {
        Ok(rd) => rd,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    let mut file = data.shared_file.write().await;
    let existing_paths: HashSet<String> = file.torrents
        .iter()
        .flat_map(|t| t.file.iter().cloned())
        .collect();
    let mut added = 0usize;
    let mut skipped = 0usize;
    for dir_entry in read_dir.filter_map(|e| e.ok()) {
        let name = dir_entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = dir_entry.path();
        let path_str = path.to_string_lossy().into_owned();
        if existing_paths.contains(&path_str) {
            skipped += 1;
            continue;
        }
        let torrent_name = if path.is_dir() {
            name.clone()
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&name)
                .to_string()
        };
        file.torrents.push(TorrentEntry {
            out: None,
            name: Some(torrent_name),
            file: vec![path_str],
            trackers: vec![],
            webseed: None,
            ice: None,
            rtc_interval: None,
            protocol: None,
            version: None,
            torrent_file: None,
            magnet: None,
            enabled: true,
            upload_limit: None,
        });
        added += 1;
    }
    if added > 0 {
        if let Err(e) = write_yaml(&data.yaml_path, &file) {
            return HttpResponse::InternalServerError().body(e.to_string());
        }
        let _ = data.reload_tx.send(());
    }
    log::info!("[Web] Batch add: {} added, {} skipped", added, skipped);
    HttpResponse::Ok().json(json!({ "added": added, "skipped": skipped }))
}

#[derive(Deserialize)]
pub struct FileUploadInitRequest {
    pub dest: String,
    pub size: u64,
    pub chunks: u32,
    pub chunk_size: u64,
    pub file_sha256: String,
}

pub async fn file_upload_init(
    req: HttpRequest,
    data: Data<AppState>,
    body: Json<FileUploadInitRequest>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    if body.chunks == 0 {
        return HttpResponse::BadRequest().json(json!({"error": "chunks must be > 0"}));
    }
    if body.chunk_size == 0 {
        return HttpResponse::BadRequest().json(json!({"error": "chunk_size must be > 0"}));
    }
    if std::path::Path::new(&body.dest)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return HttpResponse::BadRequest()
            .json(json!({"error": "Destination path must not contain '..' components"}));
    }
    const MAX_CONCURRENT_UPLOADS: usize = 20;
    {
        let uploads = data.uploads.lock().await;
        if uploads.len() >= MAX_CONCURRENT_UPLOADS {
            return HttpResponse::TooManyRequests().json(json!({
                "error": "Too many concurrent uploads. Please finalize or cancel existing uploads first."
            }));
        }
    }
    let dest = std::path::PathBuf::from(&body.dest);
    let mut part_path_os = dest.clone().into_os_string();
    part_path_os.push(".uploaded");
    let part_path = std::path::PathBuf::from(part_path_os);
    let size = body.size;
    let part_path_c = part_path.clone();
    let dest_c = dest.clone();
    let prepare = move || -> io::Result<()> {
        if let Some(parent) = dest_c.parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(e);
        }
        let file = std::fs::File::create(&part_path_c)?;
        file.set_len(size)?;
        Ok(())
    };
    match tokio::task::spawn_blocking(prepare).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return HttpResponse::InternalServerError()
            .body(format!("Failed to prepare upload file: {}", e)),
        Err(e) => return HttpResponse::InternalServerError()
            .body(format!("Task error: {}", e)),
    }
    let upload_id = generate_token();
    let session = UploadSession {
        dest,
        total_chunks: body.chunks,
        file_size: body.size,
        file_sha256: body.file_sha256.clone(),
        received: std::collections::BTreeSet::new(),
        part_path,
        chunk_size: body.chunk_size,
        hash_bytes_done: Arc::new(AtomicU64::new(0)),
    };
    data.uploads.lock().await.insert(upload_id.clone(), session);
    log::info!("[Web] File upload started: {} ({} chunks)", body.dest, body.chunks);
    HttpResponse::Ok().json(json!({"upload_id": upload_id}))
}

#[derive(Deserialize)]
pub struct FileUploadChunkQuery {
    pub id: String,
    pub n: u32,
    pub sha256: String,
}

pub async fn file_upload_chunk(
    req: HttpRequest,
    data: Data<AppState>,
    query: Query<FileUploadChunkQuery>,
    body: Bytes,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let computed = hex::encode(Sha256::digest(&body));
    if computed != query.sha256 {
        return HttpResponse::BadRequest().json(json!({
            "error": format!("SHA-256 mismatch on chunk {}: expected {}, got {}", query.n, query.sha256, computed)
        }));
    }
    let (part_path, chunk_size, total_chunks) = {
        let uploads = data.uploads.lock().await;
        let session = match uploads.get(&query.id) {
            Some(s) => s,
            None => return HttpResponse::NotFound().json(json!({"error": "Unknown upload ID"})),
        };
        if query.n >= session.total_chunks {
            return HttpResponse::BadRequest().json(json!({
                "error": format!("Chunk index {} out of range (total {})", query.n, session.total_chunks)
            }));
        }
        if body.len() as u64 > session.chunk_size {
            return HttpResponse::BadRequest().json(json!({
                "error": format!(
                    "Chunk body ({} bytes) exceeds declared chunk_size ({} bytes)",
                    body.len(), session.chunk_size
                )
            }));
        }
        (session.part_path.clone(), session.chunk_size, session.total_chunks)
    };
    let n = query.n;
    let offset = n as u64 * chunk_size;
    let write = move || -> io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new().write(true).open(&part_path)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&body)?;
        Ok(())
    };
    match tokio::task::spawn_blocking(write).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return HttpResponse::InternalServerError()
            .body(format!("Failed to write chunk: {}", e)),
        Err(e) => return HttpResponse::InternalServerError()
            .body(format!("Task error: {}", e)),
    }
    let mut uploads = data.uploads.lock().await;
    let session = match uploads.get_mut(&query.id) {
        Some(s) => s,
        None => return HttpResponse::NotFound().json(json!({"error": "Unknown upload ID"})),
    };
    session.received.insert(n);
    let received = session.received.len() as u32;
    HttpResponse::Ok().json(json!({"ok": true, "received": received, "total": total_chunks}))
}

#[derive(Deserialize)]
pub struct FileUploadFinalizeRequest {
    pub upload_id: String,
}

pub async fn file_upload_finalize(
    req: HttpRequest,
    data: Data<AppState>,
    body: Json<FileUploadFinalizeRequest>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let (dest, part_path, file_sha256, hash_progress) = {
        let uploads = data.uploads.lock().await;
        let session = match uploads.get(&body.upload_id) {
            Some(s) => s,
            None => return HttpResponse::NotFound().json(json!({"error": "Unknown upload ID"})),
        };
        let expected: std::collections::BTreeSet<u32> = (0..session.total_chunks).collect();
        if session.received != expected {
            let missing: Vec<u32> = expected.difference(&session.received).cloned().collect();
            return HttpResponse::BadRequest().json(json!({
                "error": "Missing chunks",
                "missing": missing,
            }));
        }
        (
            session.dest.clone(),
            session.part_path.clone(),
            session.file_sha256.clone(),
            Arc::clone(&session.hash_bytes_done),
        )
    };
    let dest_log = dest.clone();
    let finalize = move || -> io::Result<String> {
        use std::io::Read;
        let mut file = std::fs::File::open(&part_path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
            hash_progress.fetch_add(n as u64, Ordering::Relaxed);
        }
        let computed = hex::encode(hasher.finalize());
        if computed != file_sha256 {
            let _ = std::fs::remove_file(&part_path);
            return Err(io::Error::other(format!(
                "Integrity check failed: expected SHA-256 {file_sha256}, computed {computed}"
            )));
        }
        std::fs::rename(&part_path, &dest)?;
        Ok(computed)
    };
    let hash_result = tokio::task::spawn_blocking(finalize).await;
    data.uploads.lock().await.remove(&body.upload_id);
    let sha256 = match hash_result {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return HttpResponse::InternalServerError().body(e.to_string()),
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    log::info!("[Web] File upload finalised: {} (sha256: {})", dest_log.display(), sha256);
    HttpResponse::Ok().json(json!({"ok": true, "sha256": sha256}))
}

pub async fn file_upload_cancel(
    req: HttpRequest,
    data: Data<AppState>,
    upload_id: Path<String>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let mut uploads = data.uploads.lock().await;
    if let Some(session) = uploads.remove(upload_id.as_str()) {
        let _ = std::fs::remove_file(&session.part_path);
    }
    HttpResponse::Ok().json(json!({"ok": true}))
}

pub async fn file_upload_hash_progress(
    req: HttpRequest,
    data: Data<AppState>,
    upload_id: Path<String>,
) -> HttpResponse {
    if !is_authenticated(&req, &data).await {
        return HttpResponse::Unauthorized().json(json!({"error": "Unauthorized"}));
    }
    let uploads = data.uploads.lock().await;
    let session = match uploads.get(upload_id.as_str()) {
        Some(s) => s,
        None => return HttpResponse::NotFound().json(json!({"error": "Unknown upload ID"})),
    };
    let bytes_done = session.hash_bytes_done.load(Ordering::Relaxed);
    let total = session.file_size;
    let percent = if total > 0 {
        ((bytes_done as f64 / total as f64) * 100.0) as u32
    } else {
        100
    };
    HttpResponse::Ok().json(json!({ "bytes_done": bytes_done, "total": total, "percent": percent }))
}