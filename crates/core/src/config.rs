use crate::error::{Error, Result};
use crate::screen::ScreenLayout;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Whether the local machine acts as the controlling workstation (server)
/// or one of the controlled machines (client). Note: "server" here is the
/// machine whose keyboard / mouse is shared — i.e. the one users sit at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerRole {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub role: ServerRole,
    pub peer_name: String,
    pub network: NetworkConfig,
    pub clipboard: ClipboardConfig,
    pub file_transfer: FileTransferConfig,
    pub layout: ScreenLayout,
    pub emergency_hotkey: String,
    pub log_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            role: ServerRole::Server,
            peer_name: hostname_or_unknown(),
            network: NetworkConfig::default(),
            clipboard: ClipboardConfig::default(),
            file_transfer: FileTransferConfig::default(),
            layout: ScreenLayout::default(),
            emergency_hotkey: "Ctrl+Alt+Shift+Esc".into(),
            log_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub listen: SocketAddr,
    pub connect: Option<SocketAddr>,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub reconnect_initial_ms: u64,
    pub reconnect_max_ms: u64,
    pub trusted_fingerprints: Vec<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:24800".parse().expect("static addr"),
            connect: None,
            heartbeat_interval_ms: 2_000,
            heartbeat_timeout_ms: 6_000,
            reconnect_initial_ms: 500,
            reconnect_max_ms: 30_000,
            trusted_fingerprints: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    pub enabled: bool,
    pub max_bytes: u64,
    pub sync_images: bool,
    pub sync_files: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 100 * 1024 * 1024,
            sync_images: true,
            sync_files: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransferConfig {
    pub enabled: bool,
    pub drop_dir: Option<PathBuf>,
    pub chunk_size: u32,
    pub max_concurrent: u32,
    pub compress: bool,
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            drop_dir: None,
            chunk_size: 64 * 1024,
            max_concurrent: 4,
            compress: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::ConfigMissing(path.to_path_buf()),
            _ => Error::Io(e),
        })?;
        let cfg = toml::from_str(&text)?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// OS-specific default config path.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("org", "InputSync", "InputSync")
            .ok_or_else(|| Error::Config("could not determine config directory".into()))?;
        Ok(dirs.config_dir().join("inputsync.toml"))
    }
}

fn hostname_or_unknown() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}
