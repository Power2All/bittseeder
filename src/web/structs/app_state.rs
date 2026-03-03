use crate::config::structs::torrents_file::TorrentsFile;
use crate::stats::shared_stats::SharedStats;
use crate::web::structs::upload_session::UploadSession;
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
    RwLock,
};

pub type SessionStore = Arc<Mutex<HashMap<String, std::time::Instant>>>;
pub type UploadStore  = Arc<Mutex<HashMap<String, UploadSession>>>;

pub struct AppState {
    pub yaml_path: PathBuf,
    pub shared_file: Arc<RwLock<TorrentsFile>>,
    pub stats: SharedStats,
    pub reload_tx: watch::Sender<()>,
    pub web_password: Option<String>,
    pub sessions: SessionStore,
    pub log_tx: broadcast::Sender<String>,
    pub log_buffer: Arc<std::sync::Mutex<VecDeque<String>>>,
    pub uploads: UploadStore,
    pub login_attempts: Arc<Mutex<HashMap<std::net::IpAddr, (u32, std::time::Instant)>>>,
}