use crate::config::enums::seed_protocol::SeedProtocol;
use crate::config::structs::proxy_config::ProxyConfig;
use crate::config::structs::seeder_config::SeederConfig;
use crate::config::structs::torrent_entry::TorrentEntry;
use crate::torrent::torrent::collect_dir_files;
use crate::torrent::enums::torrent_version::TorrentVersion;
use std::path::PathBuf;

impl TorrentEntry {
    pub fn to_seeder_config(
        &self,
        proxy: Option<&ProxyConfig>,
        listen_port: u16,
        global_protocol: SeedProtocol,
        global_ice: &[String],
        global_rtc_interval_ms: u64,
    ) -> Result<SeederConfig, String> {
        if let Some(ref torrent_path) = self.torrent_file
            && !self.create_torrent
        {
            let path = PathBuf::from(torrent_path);
            if !path.exists() {
                return Err(format!("Torrent file does not exist: {}. Either create the torrent first or enable 'Create torrent if it doesn't exist'.", path.display()));
            }
        }
        if self.file.is_empty() && self.torrent_file.is_none() {
            return Err("torrent entry needs at least one file or a torrent_file path".to_string());
        }
        let mut file_paths: Vec<PathBuf> = Vec::new();
        for file_path in &self.file {
            let path = PathBuf::from(file_path);
            if path.is_dir() {
                if !path.exists() {
                    return Err(format!("Directory not found: {}", path.display()));
                }
                let mut dir_files: Vec<(PathBuf, Vec<String>)> = Vec::new();
                match collect_dir_files(&path, &path, &mut dir_files) {
                    Ok(_) => {
                        for (fp, _) in dir_files {
                            if let Some(allowed) = &self.allowed_extensions
                                && let Some(ext) = fp.extension().and_then(|e| e.to_str())
                                && !allowed.iter().any(|a| a.to_lowercase() == ext.to_lowercase()) {
                                log::debug!("[Config] Skipping {} (extension not allowed)", fp.display());
                                continue;
                            }
                            file_paths.push(fp);
                        }
                    }
                    Err(e) => {
                        log::warn!("[Config] Failed to scan directory {}: {} - skipping", path.display(), e);
                    }
                }
            } else {
                if !path.exists() {
                    return Err(format!("File not found: {}", path.display()));
                }
                if let Some(allowed) = &self.allowed_extensions
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && !allowed.iter().any(|a| a.to_lowercase() == ext.to_lowercase()) {
                    log::debug!("[Config] Skipping {} (extension not allowed)", path.display());
                    continue;
                }
                file_paths.push(path);
            }
        }
        if self.allowed_extensions.is_some() {
            if file_paths.is_empty() && self.torrent_file.is_none() && !self.file.is_empty() {
                return Err("No files found after extension filtering. Check your allowed_extensions setting.".to_string());
            }
            log::info!(
                "[Config] Processed {} file(s): found {} after extension filtering",
                self.file.len(),
                file_paths.len()
            );
        }
        let out_file = if self.create_torrent {
            self.torrent_file.as_ref().map(PathBuf::from)
        } else {
            None
        };
        let webseed_urls = self.webseed.clone().unwrap_or_default();
        let protocol = self.protocol.clone().unwrap_or(global_protocol);
        let ice_servers = self.ice.clone().unwrap_or_else(|| {
            if global_ice.is_empty() {
                vec![
                    "stun:stun.l.google.com:19302".to_string(),
                    "stun:stun1.l.google.com:19302".to_string(),
                ]
            } else {
                global_ice.to_vec()
            }
        });
        let rtc_interval_ms = self.rtc_interval
            .map(|s| s * 1000)
            .unwrap_or(global_rtc_interval_ms);
        let version = match self.version.as_deref() {
            Some("v2") => TorrentVersion::V2,
            Some("hybrid") => TorrentVersion::Hybrid,
            _ => TorrentVersion::V1,
        };
        Ok(SeederConfig {
            tracker_urls: self.trackers.clone(),
            file_paths,
            name: self.name.clone(),
            out_file,
            webseed_urls,
            listen_port,
            upnp: false,
            ice_servers,
            rtc_interval_ms,
            protocol,
            version,
            torrent_file: self.torrent_file.as_ref().map(PathBuf::from),
            magnet: self.magnet.clone(),
            upload_limit: self.upload_limit,
            proxy: proxy.cloned(),
            show_stats: true,
            private: self.private,
        })
    }
}