use crate::app::DaemonState;
use inputsync_ipc::{IpcListener, IpcRequest, IpcResponse, PeerSummary, StatusReply};
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
                        let resp = handle_request(req, &state);
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

fn handle_request(req: IpcRequest, state: &Arc<parking_lot::RwLock<DaemonState>>) -> IpcResponse {
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
            })
        }
        IpcRequest::GetConfig => IpcResponse::Config(state.read().config.clone()),
        IpcRequest::UpdateConfig { config } => {
            state.write().config = config;
            IpcResponse::Ok
        }
        IpcRequest::Connect { addr } => {
            // For v1, surfaces the request to ops as a TODO — actually
            // dialing requires re-spinning up the network task.
            tracing::info!(target_addr = %addr, "ipc: connect request");
            IpcResponse::Error {
                message: "runtime peer add not yet implemented".into(),
            }
        }
        IpcRequest::Disconnect { peer } => {
            tracing::info!(%peer, "ipc: disconnect request");
            IpcResponse::Error {
                message: "runtime peer remove not yet implemented".into(),
            }
        }
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
