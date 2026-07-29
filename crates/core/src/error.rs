use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("config file not found at {0}")]
    ConfigMissing(PathBuf),

    #[error("toml parse error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("platform error: {0}")]
    Platform(String),

    #[error("ipc error: {0}")]
    Ipc(String),

    #[error("peer rejected: {0}")]
    PeerRejected(String),

    #[error("version mismatch: local={local} remote={remote}")]
    VersionMismatch { local: u16, remote: u16 },

    #[error("emergency stop activated")]
    EmergencyStop,

    #[error("operation cancelled")]
    Cancelled,

    #[error("not connected")]
    NotConnected,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
