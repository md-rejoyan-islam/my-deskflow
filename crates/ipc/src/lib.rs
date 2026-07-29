//! Daemon ↔ GUI / CLI inter-process communication.
//!
//! Wire format: newline-delimited JSON. Each line is one [`IpcRequest`] or
//! [`IpcResponse`] / [`IpcEvent`]. Simple and inspectable with `socat` for
//! debugging.

use inputsync_core::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod transport;

pub use transport::{
    default_socket_path, listen, IpcClient, IpcConnection, IpcListener, IpcRead, IpcWrite,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcRequest {
    GetStatus,
    GetConfig,
    UpdateConfig { config: Config },
    Connect { addr: String },
    Disconnect { peer: String },
    EmergencyStop,
    SubscribeEvents,
    GetLogs { tail: u32 },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcResponse {
    Status(StatusReply),
    Config(Config),
    Ok,
    Logs { lines: Vec<String> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReply {
    pub version: String,
    pub uptime_secs: u64,
    pub role: String,
    pub local_fingerprint: String,
    pub connected_peers: Vec<PeerSummary>,
    pub capturing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSummary {
    pub peer_id: String,
    pub name: String,
    pub remote_addr: String,
    pub connected_secs: u64,
    pub last_rtt_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcEvent {
    PeerConnected {
        peer_id: String,
        name: String,
    },
    PeerDisconnected {
        peer_id: String,
    },
    CapturingChanged {
        capturing: bool,
    },
    FileTransferProgress {
        transfer_id: u64,
        bytes: u64,
        total: u64,
    },
    Log {
        line: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IpcMessage {
    Response(IpcResponse),
    Event(IpcEvent),
}

#[derive(Debug, Clone)]
pub struct SocketPaths {
    pub socket: PathBuf,
}
