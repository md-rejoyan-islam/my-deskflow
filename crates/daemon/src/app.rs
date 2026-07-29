//! Top-level daemon orchestration. Wires platform, network, IPC, and
//! session orchestrator together.

use crate::session::Session;
use anyhow::{anyhow, Context, Result};
use inputsync_core::{Config, PeerId, ServerRole};
use inputsync_network::{
    tls, ClientConfig, ClientController, ClientEvent, Server, ServerConfig, ServerEvent,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, warn};

pub struct RunArgs {
    pub config_path: Option<PathBuf>,
    pub role_override: Option<String>,
    pub listen_override: Option<SocketAddr>,
    pub connect_override: Option<SocketAddr>,
    pub pinned_fingerprints: Vec<String>,
    pub ipc_enabled: bool,
}

pub async fn run(args: RunArgs) -> Result<()> {
    let config_path = match args.config_path.clone() {
        Some(p) => p,
        None => Config::default_path().context("default config path")?,
    };

    let mut config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(inputsync_core::Error::ConfigMissing(p)) => {
            info!(path = %p.display(), "no config found, using defaults");
            Config::default()
        }
        Err(e) => return Err(e.into()),
    };

    if let Some(role) = args.role_override.as_deref() {
        config.role = match role.to_ascii_lowercase().as_str() {
            "server" => ServerRole::Server,
            "client" => ServerRole::Client,
            other => return Err(anyhow!("unknown role '{other}'; expected server or client")),
        };
    }
    if let Some(l) = args.listen_override {
        config.network.listen = l;
    }
    if let Some(c) = args.connect_override {
        config.network.connect = Some(c);
    }
    if !args.pinned_fingerprints.is_empty() {
        config.network.trusted_fingerprints = args.pinned_fingerprints.clone();
    }

    let cert_dir = cert_dir().context("locate cert dir")?;
    let cert = tls::load_or_generate(&cert_dir).context("load/generate cert")?;
    info!(fingerprint = %cert.fingerprint_hex, "local cert fingerprint");

    let started = Instant::now();
    let local_peer_id = PeerId::new();

    let platform = Arc::<dyn inputsync_platform::Platform>::from(
        inputsync_platform::current().context("platform init")?,
    );

    let runtime_state = Arc::new(parking_lot::RwLock::new(DaemonState {
        config: config.clone(),
        started,
        capturing: false,
        fingerprint: cert.fingerprint_hex.clone(),
        peers: Vec::new(),
        client_controller: None,
        client_base: None,
    }));

    // IPC server.
    let ipc_handle = if args.ipc_enabled {
        let socket = inputsync_ipc::default_socket_path().context("ipc socket path")?;
        let listener = inputsync_ipc::listen(&socket).context("bind ipc")?;
        info!(path = %socket.display(), "ipc listening");
        Some(tokio::spawn(crate::ipc_server::run(
            listener,
            runtime_state.clone(),
        )))
    } else {
        None
    };

    let session = Arc::new(Session::new(
        config.role,
        platform.clone(),
        config.clone(),
        local_peer_id,
    ));
    let runtime_state_for_session = runtime_state.clone();

    // Network role: server or client.
    let network_task = match config.role {
        ServerRole::Server => {
            let server_cfg = ServerConfig {
                listen: config.network.listen,
                local_peer_id,
                local_peer_name: config.peer_name.clone(),
                cert,
            };
            let server = Server::bind(server_cfg).context("bind quic server")?;
            let local_addr = server.local_addr().ok();
            info!(addr = ?local_addr, "running as server");

            let server_session = session.clone().spawn_server();
            let (raw_tx, mut raw_rx) = mpsc::channel(64);

            let net = tokio::spawn(async move {
                if let Err(e) = server.run(raw_tx).await {
                    warn!(error = %e, "server task exited");
                }
            });

            // Fan-out: track peers in DaemonState and forward to session.
            let session_tx = server_session.peer_tx.clone();
            let state = runtime_state_for_session.clone();
            tokio::spawn(async move {
                while let Some(evt) = raw_rx.recv().await {
                    match &evt {
                        ServerEvent::Connected { handle, .. } => {
                            state.write().peers.push(TrackedPeer {
                                peer_id: handle.peer_id,
                                name: handle.peer_name.clone(),
                                remote_addr: handle
                                    .remote_addr
                                    .map(|a| a.to_string())
                                    .unwrap_or_default(),
                                connected_at: Instant::now(),
                                last_rtt_ms: 0,
                            });
                        }
                        ServerEvent::Disconnected { peer_id } => {
                            state.write().peers.retain(|p| p.peer_id != *peer_id);
                        }
                    }
                    if session_tx.send(evt).await.is_err() {
                        break;
                    }
                }
            });
            net
        }
        ServerRole::Client => {
            // `connect` is now optional: `--role client` with no --connect
            // starts the daemon idle, ready for a GUI-driven Dial.
            let initial_connect = config.network.connect;

            // Build the base client config (without a concrete server_addr; it
            // is filled in per-dial). A zero addr is a placeholder; the IPC
            // Connect handler overrides it.
            let client_base = ClientConfig {
                server_addr: initial_connect.unwrap_or_else(|| "0.0.0.0:0".parse().unwrap()),
                local_peer_id,
                local_peer_name: config.peer_name.clone(),
                trusted_fingerprints: config.network.trusted_fingerprints.clone(),
                heartbeat_interval_ms: config.network.heartbeat_interval_ms,
            };
            info!(connect = ?initial_connect, "running as client");

            // The controller owns the long-lived QUIC endpoint + supervisor.
            let controller = Arc::new(
                ClientController::new().map_err(|e| anyhow!("client controller bind: {e}"))?,
            );

            // Take the events receiver BEFORE moving the controller into state.
            let mut events_rx = controller.take_events().expect("events rx");

            // Expose controller + base config to the IPC layer (GUI Connect/Disconnect).
            {
                let mut s = runtime_state.write();
                s.client_controller = Some(controller.clone());
                s.client_base = Some(client_base.clone());
            }

            let client_session = session.clone().spawn_client();
            let session_tx = client_session.peer_tx.clone();
            let state = runtime_state_for_session.clone();

            // Initial auto-dial if a connect address was configured at startup
            // (preserves the original `--connect` behavior).
            if let Some(addr) = initial_connect {
                let mut first_cfg = client_base.clone();
                first_cfg.server_addr = addr;
                let ctrl = controller.clone();
                tokio::spawn(async move {
                    ctrl.dial(first_cfg).await;
                });
            }

            let net = tokio::spawn(async move {
                // Drain controller events -> update DaemonState.peers and
                // forward to the session. This replaces the per-attempt
                // forwarder task; there is now exactly one for the daemon's
                // lifetime (reconnection is GUI-driven, not automatic).
                while let Some(evt) = events_rx.recv().await {
                    match &evt {
                        ClientEvent::Connected { handle, .. } => {
                            state.write().peers.push(TrackedPeer {
                                peer_id: handle.peer_id,
                                name: handle.peer_name.clone(),
                                remote_addr: handle
                                    .remote_addr
                                    .map(|a| a.to_string())
                                    .unwrap_or_default(),
                                connected_at: Instant::now(),
                                last_rtt_ms: 0,
                            });
                        }
                        ClientEvent::Disconnected { peer_id } => {
                            state.write().peers.retain(|p| p.peer_id != *peer_id);
                        }
                    }
                    if session_tx.send(evt).await.is_err() {
                        break;
                    }
                }
            });
            net
        }
    };

    info!("daemon ready");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl-c received, shutting down");
        }
        _ = network_task => {}
    }

    if let Some(h) = ipc_handle {
        h.abort();
    }

    Ok(())
}

pub fn init_config(path: Option<PathBuf>, force: bool) -> Result<()> {
    let path = match path {
        Some(p) => p,
        None => Config::default_path().context("default config path")?,
    };
    if path.exists() && !force {
        return Err(anyhow!(
            "config already exists at {}; pass --force to overwrite",
            path.display()
        ));
    }
    let cfg = Config::default();
    cfg.save(&path).context("save default config")?;
    println!("wrote default config to {}", path.display());
    Ok(())
}

pub fn print_fingerprint(_config_path: Option<PathBuf>) -> Result<()> {
    let dir = cert_dir().context("locate cert dir")?;
    let bundle = tls::load_or_generate(&dir).context("load/generate cert")?;
    println!("{}", bundle.fingerprint_hex);
    Ok(())
}

#[cfg(any(windows, target_os = "linux"))]
pub fn install_service() -> Result<()> {
    println!("service installation is a packaging-time step; see packaging/ in the repo.");
    Ok(())
}

#[cfg(any(windows, target_os = "linux"))]
pub fn uninstall_service() -> Result<()> {
    println!("service removal is a packaging-time step; see packaging/ in the repo.");
    Ok(())
}

fn cert_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("org", "InputSync", "InputSync")
        .ok_or_else(|| anyhow!("could not locate project dirs"))?;
    Ok(dirs.data_dir().join("certs"))
}

pub struct DaemonState {
    pub config: Config,
    pub started: Instant,
    pub capturing: bool,
    pub fingerprint: String,
    pub peers: Vec<TrackedPeer>,
    /// Present only when the daemon runs in client role. The IPC layer uses
    /// this to satisfy GUI-driven Connect/Disconnect requests.
    pub client_controller: Option<Arc<ClientController>>,
    /// The client role in config (copied at startup) so the IPC layer can
    /// build a fresh `ClientConfig` per dial without re-reading config.
    pub client_base: Option<ClientConfig>,
}

#[derive(Clone)]
pub struct TrackedPeer {
    pub peer_id: PeerId,
    pub name: String,
    pub remote_addr: String,
    pub connected_at: Instant,
    pub last_rtt_ms: u32,
}
