use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

pub struct UploadSession {
    pub dest: PathBuf,
    pub total_chunks: u32,
    pub file_size: u64,
    pub file_sha256: String,
    pub received: BTreeSet<u32>,
    pub part_path: PathBuf,
    pub chunk_size: u64,
    pub hash_bytes_done: Arc<AtomicU64>,
}