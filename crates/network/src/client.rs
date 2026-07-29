//! QUIC client.

use crate::peer::PeerHandle;
use crate::tls;
use inputsync_core::{Error, PeerId, Result};
use inputsync_protocol::{Capabilities, Hello, Message};
use quinn::{ClientConfig as QuinnClientConfig, Endpoint};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct ClientConfig {
    pub server_addr: SocketAddr,
    pub local_peer_id: PeerId,
    pub local_peer_name: String,
    pub trusted_fingerprints: Vec<String>,
    pub heartbeat_interval_ms: u64,
}

pub struct Client {
    cfg: ClientConfig,
}

impl Client {
    pub fn new(cfg: ClientConfig) -> Self {
        Self { cfg }
    }

    pub fn resolve(host: &str) -> Result<SocketAddr> {
        host.to_socket_addrs()
            .map_err(|e| Error::Network(format!("resolve {host}: {e}")))?
            .next()
            .ok_or_else(|| Error::Network(format!("no addresses for {host}")))
    }

    /// One-shot connection: bind a fresh endpoint, dial, run the peer loop,
    /// then return when the connection ends. Kept for back-compat with the
    /// original CLI-driven flow. The GUI-driven flow uses `ClientController`,
    /// which reuses a single endpoint across dials.
    pub async fn run(self, events: mpsc::Sender<ClientEvent>) -> Result<()> {
        let bind: SocketAddr = if self.cfg.server_addr.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        let mut endpoint =
            Endpoint::client(bind).map_err(|e| Error::Network(format!("client bind: {e}")))?;
        // `connect_once` owns the TLS setup + dial + handshake + peer loop.
        connect_once(&mut endpoint, &self.cfg, events).await
    }
}

/// Perform exactly one connection attempt against `cfg.server_addr` using the
/// supplied `endpoint`. Runs the peer loop to completion, then emits
/// `ClientEvent::Disconnected` and returns.
///
/// The endpoint's default client config is (re)configured with the
/// fingerprints from `cfg` on every call, so dials to different servers (with
/// different trusted pins) work from a shared endpoint.
pub async fn connect_once(
    endpoint: &mut Endpoint,
    cfg: &ClientConfig,
    events: mpsc::Sender<ClientEvent>,
) -> Result<()> {
    // (Re)configure TLS for this dial — trusted_fingerprints may differ per
    // server, so we cannot set this once at endpoint construction.
    let client_tls = tls::client_rustls_config(Some(cfg.trusted_fingerprints.clone()))?;
    let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(client_tls)
        .map_err(|e| Error::Network(format!("quic client: {e}")))?;
    endpoint.set_default_client_config(QuinnClientConfig::new(Arc::new(qcc)));

    tracing::info!(addr = %cfg.server_addr, "connecting");
    let connection = endpoint
        .connect(cfg.server_addr, "inputsync.local")
        .map_err(|e| Error::Network(format!("connect: {e}")))?
        .await
        .map_err(|e| Error::Network(format!("handshake: {e}")))?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| Error::Network(format!("open_bi: {e}")))?;

    let hello = Message::Hello(Hello {
        peer_id: cfg.local_peer_id,
        peer_name: cfg.local_peer_name.clone(),
        protocol_version: inputsync_protocol::PROTOCOL_VERSION,
        capabilities: Capabilities::full(),
    });
    crate::stream::write_message(&mut send, &hello).await?;

    let welcome = match crate::stream::read_message(&mut recv).await? {
        Message::Welcome(w) => w,
        Message::Error { code, message } => {
            return Err(Error::PeerRejected(format!("[{code}] {message}")))
        }
        other => {
            return Err(Error::Protocol(format!(
                "expected Welcome, got {:?}",
                other.message_type()
            )))
        }
    };

    let remote = connection.remote_address();
    let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(256);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Message>(256);

    let handle = PeerHandle::new(
        welcome.peer_id,
        welcome.peer_name.clone(),
        Some(remote),
        connection.clone(),
        outbound_tx,
    );

    let _ = events
        .send(ClientEvent::Connected {
            handle: handle.clone(),
            inbound: inbound_rx,
        })
        .await;

    crate::peer_loop::run_peer_loop(connection, send, recv, outbound_rx, inbound_tx).await;

    handle.clear().await;
    let _ = events
        .send(ClientEvent::Disconnected {
            peer_id: welcome.peer_id,
        })
        .await;
    Ok(())
}

/// Commands the supervisor task accepts.
pub enum ClientCommand {
    /// Abort any current connection and dial `cfg`'s server.
    Dial(ClientConfig),
    /// Abort the current connection (if any).
    HangUp,
}

/// Runtime-controllable client. Holds one long-lived QUIC endpoint and a
/// supervisor task that owns the (at most one) active connection.
///
/// Created once by the daemon when it starts in client mode; the IPC layer
/// calls `dial` / `hang_up` in response to GUI requests. `ClientEvent`s flow
/// out of `events_rx` (obtained via `take_events`) for the daemon to track
/// peers and forward to the session.
pub struct ClientController {
    tx: mpsc::Sender<ClientCommand>,
    events_rx: parking_lot::Mutex<Option<mpsc::Receiver<ClientEvent>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl ClientController {
    /// Bind a client endpoint and spawn the supervisor. `events` produced by
    /// connections are obtainable via [`ClientController::take_events`].
    pub fn new() -> std::io::Result<Self> {
        // A single ephemeral local endpoint is reused across all dials. The
        // endpoint is MOVED into the supervisor task, which is the sole owner;
        // this avoids any need for interior sharing or unsafe statics.
        let endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientCommand>(16);
        let (events_tx, events_rx) = mpsc::channel::<ClientEvent>(64);

        let task = tokio::spawn(async move {
            let mut endpoint = endpoint;
            // `pending` holds a Dial that should (re)start next. It is set
            // when a Dial arrives while a connection is already running
            // (causing the running one to be cancelled) and consumed at the
            // top of each pass through the loop.
            let mut pending: Option<ClientConfig> = None;

            loop {
                // Decide what to dial next: a pending dial, or wait for one.
                let dial_cfg = match pending.take() {
                    Some(cfg) => cfg,
                    None => match cmd_rx.recv().await {
                        Some(ClientCommand::Dial(cfg)) => cfg,
                        // HangUp with no active connection is a no-op.
                        Some(ClientCommand::HangUp) => continue,
                        // Controller dropped -> exit the supervisor.
                        None => return,
                    },
                };

                tracing::info!(addr = %dial_cfg.server_addr, "supervisor: dialing");
                let ev = events_tx.clone();
                // Race the connection against the next incoming command. If a
                // command wins, the connect_once future is dropped, which
                // cancels the in-flight handshake / peer loop cleanly.
                tokio::select! {
                    biased;
                    cmd = cmd_rx.recv() => match cmd {
                        Some(ClientCommand::Dial(cfg)) => {
                            // Cancel current; start cfg on the next loop pass.
                            tracing::info!("supervisor: re-dial requested, cancelling current");
                            pending = Some(cfg);
                        }
                        Some(ClientCommand::HangUp) => {
                            tracing::info!("supervisor: hang-up requested, cancelling current");
                        }
                        None => return,
                    },
                    r = connect_once(&mut endpoint, &dial_cfg, ev) => {
                        if let Err(e) = r {
                            tracing::warn!(error = %e, "client connection ended");
                        }
                        // Connection finished (cleanly or with error). Loop back;
                        // if `pending` is empty we'll block waiting for a Dial.
                    }
                }
            }
        });

        Ok(Self {
            tx: cmd_tx,
            events_rx: parking_lot::Mutex::new(Some(events_rx)),
            _task: task,
        })
    }

    /// Take ownership of the events receiver. Should be called exactly once
    /// right after construction so the daemon can drain peer events.
    pub fn take_events(&self) -> Option<mpsc::Receiver<ClientEvent>> {
        self.events_rx.lock().take()
    }

    /// Dial the server described by `cfg`. Replaces any current connection.
    pub async fn dial(&self, cfg: ClientConfig) {
        let _ = self.tx.send(ClientCommand::Dial(cfg)).await;
    }

    /// Hang up the current connection, if any.
    pub async fn hang_up(&self) {
        let _ = self.tx.send(ClientCommand::HangUp).await;
    }
}

pub enum ClientEvent {
    Connected {
        handle: PeerHandle,
        inbound: mpsc::Receiver<Message>,
    },
    Disconnected {
        peer_id: PeerId,
    },
}
