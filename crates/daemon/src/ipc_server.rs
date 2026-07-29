use crate::app::{self, DaemonState};
use inputsync_ipc::{IpcListener, IpcRequest, IpcResponse, PeerSummary, StatusReply};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tracing::warn;

pub async fn run(listener: IpcListener, state: Arc<parking_lot::RwLock<DaemonState>>) {
    loop {
        match listener.accept().await {
            Ok(mut conn) => {
                let state = state.clone();
                tokio::spawn(async move {
                    loop {
                        let req = match conn.read_request().await {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::debug!(error = %e, "ipc client disconnected");
                                return;
                            }
                        };
                        let resp = handle_request(req, &state).await;
                        if let Err(e) = conn.write_response(&resp).await {
                            warn!(error = %e, "failed to write ipc response");
                            return;
                        }
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "ipc accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

/// Most requests are synchronous; StartServer/StopServer touch async teardown
/// so the handler is async.
async fn handle_request(
    req: IpcRequest,
    state: &Arc<parking_lot::RwLock<DaemonState>>,
) -> IpcResponse {
    match req {
        IpcRequest::GetStatus => {
            let s = state.read();
            let peers = s
                .peers
                .iter()
                .map(|p| PeerSummary {
                    peer_id: p.peer_id.to_string(),
                    name: p.name.clone(),
                    remote_addr: p.remote_addr.clone(),
                    connected_secs: p.connected_at.elapsed().as_secs(),
                    last_rtt_ms: p.last_rtt_ms,
                })
                .collect();
            IpcResponse::Status(StatusReply {
                version: env!("CARGO_PKG_VERSION").into(),
                uptime_secs: s.started.elapsed().as_secs(),
                role: format!("{:?}", s.config.role).to_lowercase(),
                local_fingerprint: s.fingerprint.clone(),
                connected_peers: peers,
                capturing: s.capturing,
                listening: s.listening,
                listen_addr: s.listen_addr.map(|a| a.to_string()),
            })
        }
        IpcRequest::GetConfig => IpcResponse::Config(state.read().config.clone()),
        IpcRequest::UpdateConfig { config } => {
            state.write().config = config;
            IpcResponse::Ok
        }
        IpcRequest::Connect { addr, fingerprint } => handle_connect(addr, fingerprint, state),
        IpcRequest::Disconnect { peer } => {
            tracing::info!(%peer, "ipc: disconnect request");
            // The current controller model supports a single active client
            // connection, so `peer` is informational. Hang up regardless.
            let s = state.read();
            match &s.client_controller {
                Some(controller) => {
                    let controller = controller.clone();
                    tokio::spawn(async move {
                        controller.hang_up().await;
                    });
                    IpcResponse::Ok
                }
                None => IpcResponse::Error {
                    message: "not running as a client; nothing to disconnect".into(),
                },
            }
        }
        IpcRequest::StartServer => match app::start_server(state) {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error {
                message: format!("{e:#}"),
            },
        },
        IpcRequest::StopServer => match app::stop_server(state).await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error {
                message: format!("{e:#}"),
            },
        },
        IpcRequest::EmergencyStop => {
            state.write().capturing = false;
            IpcResponse::Ok
        }
        IpcRequest::SubscribeEvents => IpcResponse::Error {
            message: "event streaming not yet implemented".into(),
        },
        IpcRequest::GetLogs { tail: _ } => IpcResponse::Logs { lines: vec![] },
        IpcRequest::Shutdown => {
            tracing::warn!("ipc: shutdown request");
            std::process::exit(0);
        }
    }
}

/// Build a `ClientConfig` from the daemon's base config + the request's
/// address/fingerprint, then ask the controller to dial it.
///
/// Returns `Ok` once the dial is *dispatched* (the actual handshake runs
/// asynchronously; watch `GetStatus` for the resulting peer).
fn handle_connect(
    addr: String,
    fingerprint: Option<String>,
    state: &Arc<parking_lot::RwLock<DaemonState>>,
) -> IpcResponse {
    // Parse the target address. Accept either a bare SocketAddr or a
    // host:port that needs DNS resolution via the network crate's Client.
    let target: SocketAddr =
        match SocketAddr::from_str(&addr).or_else(|_| inputsync_network::Client::resolve(&addr)) {
            Ok(a) => a,
            Err(e) => {
                return IpcResponse::Error {
                    message: format!("invalid address '{addr}': {e}"),
                }
            }
        };

    let s = state.read();
    let (Some(controller), Some(mut cfg)) = (&s.client_controller, s.client_base.clone()) else {
        return IpcResponse::Error {
            message: "daemon is not running as a client; cannot connect".into(),
        };
    };
    cfg.server_addr = target;
    if let Some(fp) = fingerprint {
        let fp = fp.trim().to_string();
        if !fp.is_empty() {
            cfg.trusted_fingerprints = vec![fp];
        }
    }
    let controller = controller.clone();
    tracing::info!(addr = %target, "ipc: connect request, dispatching dial");
    tokio::spawn(async move {
        controller.dial(cfg).await;
    });
    IpcResponse::Ok
}
