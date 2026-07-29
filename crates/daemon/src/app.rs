//! Top-level daemon orchestration. Wires platform, network, IPC, and
//! session orchestrator together.

use crate::session::Session;
use anyhow::{anyhow, Context, Result};
use inputsync_core::{Config, PeerId, ServerRole};
use inputsync_network::{
    close_endpoint, tls, ClientConfig, ClientController, ClientEvent, Endpoint, Server,
    ServerConfig, ServerEvent,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
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
        server_controller: None,
        listening: false,
        listen_addr: None,
        server_factory: None,
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

    // Network role: server or client.
    //
    // SERVER starts IDLE: no QUIC listener, no input capture. The GUI's
    // "Run" button sends StartServer over IPC, which binds the endpoint +
    // spawns the session tree. This gives the user explicit control over
    // when scanning begins.
    //
    // CLIENT starts idle too (existing behavior): no auto-dial unless a
    // --connect address was given at startup.
    let network_task: JoinHandle<()> = match config.role {
        ServerRole::Server => {
            let server_cfg = ServerConfig {
                listen: config.network.listen,
                local_peer_id,
                local_peer_name: config.peer_name.clone(),
                cert,
            };
            info!(role = "server", "running as server (idle until GUI Run)");

            // Stash the factory so the IPC StartServer handler can bring the
            // server up on demand.
            {
                let mut s = runtime_state.write();
                s.server_factory = Some(ServerFactory {
                    session: session.clone(),
                    runtime_state: runtime_state.clone(),
                    server_cfg,
                });
            }

            // Idle keep-alive: the daemon stays alive waiting for IPC commands.
            // This task never completes on its own; shutdown is ctrl-c driven.
            tokio::spawn(async move {
                let (_tx, mut rx) = mpsc::channel::<()>(1);
                let _ = rx.recv().await;
            })
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
            let state = runtime_state.clone();

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

            tokio::spawn(async move {
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
            })
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

/// Bring the QUIC server up from idle. Called by the IPC `StartServer`
/// handler (GUI "Run" button). Binds the endpoint, spawns the session tree
/// (capture + edge detection + peer fan-out), and records a `ServerController`
/// in `DaemonState` so a later `StopServer` can tear it all down.
pub fn start_server(state: &Arc<parking_lot::RwLock<DaemonState>>) -> Result<()> {
    // Read everything we need under one lock.
    let (session, server_cfg, runtime_state) = {
        let s = state.read();
        if s.server_controller.is_some() {
            return Err(anyhow!("server is already running"));
        }
        let f = s
            .server_factory
            .as_ref()
            .ok_or_else(|| anyhow!("daemon is not running as a server"))?;
        (
            f.session.clone(),
            f.server_cfg.clone(),
            f.runtime_state.clone(),
        )
    };

    // Bind the QUIC endpoint. We keep an Endpoint clone to close on StopServer.
    let (server, endpoint) = Server::bind(server_cfg).context("bind quic server")?;
    let local_addr = server.local_addr().ok();
    info!(addr = ?local_addr, "server started (listening)");

    // Spawn the session tree (capture, edge detector, peer handling).
    let server_session = session.spawn_server();
    let (raw_tx, mut raw_rx) = mpsc::channel(64);

    // Accept loop task.
    let accept_task = tokio::spawn(async move {
        if let Err(e) = server.run(raw_tx).await {
            warn!(error = %e, "server task exited");
        }
    });

    // Fan-out task: track peers in DaemonState and forward to the session.
    let session_tx = server_session.peer_tx.clone();
    let fanout_task = tokio::spawn(async move {
        while let Some(evt) = raw_rx.recv().await {
            match &evt {
                ServerEvent::Connected { handle, .. } => {
                    runtime_state.write().peers.push(TrackedPeer {
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
                    runtime_state
                        .write()
                        .peers
                        .retain(|p| p.peer_id != *peer_id);
                }
            }
            if session_tx.send(evt).await.is_err() {
                break;
            }
        }
    });

    // Record the controller so StopServer can tear it down.
    {
        let mut s = state.write();
        s.server_controller = Some(Arc::new(ServerController {
            endpoint,
            tasks: vec![accept_task, fanout_task],
        }));
        s.listening = true;
        s.capturing = true;
        s.listen_addr = local_addr;
    }
    Ok(())
}

/// Tear the QUIC server down to idle. Called by the IPC `StopServer` handler
/// (GUI "Stop" button). Idempotent: returns Ok if not running.
pub async fn stop_server(state: &Arc<parking_lot::RwLock<DaemonState>>) -> Result<()> {
    let controller = {
        let mut s = state.write();
        s.listening = false;
        s.capturing = false;
        s.listen_addr = None;
        s.peers.clear();
        s.server_controller.take()
    };
    if let Some(ctrl) = controller {
        // Arc.try_unwrap should succeed since we took the only DaemonState ref.
        match Arc::try_unwrap(ctrl) {
            Ok(c) => c.stop().await,
            Err(arc) => {
                // Fallback: close via a cloned endpoint handle (Endpoint is Clone).
                close_endpoint(&arc.endpoint);
                for t in &arc.tasks {
                    t.abort();
                }
            }
        }
        info!("server stopped (idle)");
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
    /// Present only when the server is actively listening (server role). The
    /// IPC layer uses this to satisfy GUI-driven Run/Stop (StartServer/
    /// StopServer) requests. None = daemon started idle.
    pub server_controller: Option<Arc<ServerController>>,
    /// True while the server is listening for clients (Set by StartServer,
    /// cleared by StopServer). Reported to the GUI so it can render
    /// "scanning" vs "idle".
    pub listening: bool,
    /// The bound server address, if listening. Shown to the user.
    pub listen_addr: Option<SocketAddr>,
    /// Shared handles the IPC layer needs to (re)start the server on demand.
    pub server_factory: Option<ServerFactory>,
}

/// Everything the IPC `StartServer` handler needs to bring up the QUIC
/// server + session. Built once at daemon startup; consumed per-run.
pub struct ServerFactory {
    pub session: Arc<Session>,
    pub runtime_state: Arc<parking_lot::RwLock<DaemonState>>,
    pub server_cfg: ServerConfig,
}

/// Owns the live QUIC server so it can be torn down via StopServer. Mirrors
/// the client-side `ClientController` pattern.
pub struct ServerController {
    /// Clone of the endpoint; `close()` shuts the accept loop in `Server::run`.
    pub endpoint: Endpoint,
    /// Tasks spawned by StartServer — aborted on StopServer.
    pub tasks: Vec<JoinHandle<()>>,
}

impl ServerController {
    /// Gracefully stop the server: close the endpoint, abort all tasks.
    pub async fn stop(self) {
        close_endpoint(&self.endpoint);
        for t in self.tasks {
            t.abort();
        }
    }
}

#[derive(Clone)]
pub struct TrackedPeer {
    pub peer_id: PeerId,
    pub name: String,
    pub remote_addr: String,
    pub connected_at: Instant,
    pub last_rtt_ms: u32,
}
