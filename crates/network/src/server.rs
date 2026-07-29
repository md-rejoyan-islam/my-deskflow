//! QUIC server (listener side).

use crate::peer::PeerHandle;
use crate::tls::{self, CertBundle};
use inputsync_core::{Error, PeerId, Result};
use inputsync_protocol::{Capabilities, Message, Welcome};
use quinn::{Endpoint, ServerConfig as QuinnServerConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub local_peer_id: PeerId,
    pub local_peer_name: String,
    pub cert: CertBundle,
}

pub struct Server {
    endpoint: Endpoint,
    fingerprint: String,
    cfg: ServerConfig,
}

impl Server {
    /// Bind the QUIC server endpoint. Returns the `Server` (to run the accept
    /// loop) AND a clone of the underlying `Endpoint`, which the caller keeps
    /// so it can call `endpoint.close(...)` to gracefully shut the server down
    /// — `run()` consumes `self`, so the clone is the only handle left.
    pub fn bind(cfg: ServerConfig) -> Result<(Self, Endpoint)> {
        let rustls_cfg = tls::server_rustls_config(&cfg.cert)?;
        let qsc = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_cfg)
            .map_err(|e| Error::Network(format!("quic server: {e}")))?;
        let mut server_cfg = QuinnServerConfig::with_crypto(Arc::new(qsc));
        Arc::get_mut(&mut server_cfg.transport)
            .expect("fresh transport")
            .max_concurrent_uni_streams(64u32.into())
            .max_concurrent_bidi_streams(64u32.into());

        let endpoint = Endpoint::server(server_cfg, cfg.listen)
            .map_err(|e| Error::Network(format!("bind: {e}")))?;

        let fingerprint = cfg.cert.fingerprint_hex.clone();
        tracing::info!(addr = %cfg.listen, fingerprint = %fingerprint, "QUIC server listening");
        Ok((
            Self {
                endpoint: endpoint.clone(),
                fingerprint,
                cfg,
            },
            endpoint,
        ))
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint
            .local_addr()
            .map_err(|e| Error::Network(format!("local_addr: {e}")))
    }

    pub async fn run(self, events: mpsc::Sender<ServerEvent>) -> Result<()> {
        while let Some(incoming) = self.endpoint.accept().await {
            let events = events.clone();
            let cfg = self.cfg.clone();
            tokio::spawn(async move {
                match incoming.await {
                    Ok(conn) => {
                        if let Err(e) = handle_connection(conn, cfg, events).await {
                            tracing::warn!(error = %e, "connection terminated with error");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "connection failed during handshake");
                    }
                }
            });
        }
        Ok(())
    }
}

async fn handle_connection(
    conn: quinn::Connection,
    cfg: ServerConfig,
    events: mpsc::Sender<ServerEvent>,
) -> Result<()> {
    let remote = conn.remote_address();
    tracing::info!(%remote, "client connected");

    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| Error::Network(format!("accept_bi: {e}")))?;

    let hello = match crate::stream::read_message(&mut recv).await? {
        Message::Hello(h) => h,
        other => {
            return Err(Error::Protocol(format!(
                "expected Hello, got {:?}",
                other.message_type()
            )));
        }
    };

    if hello.protocol_version != inputsync_protocol::PROTOCOL_VERSION {
        let _ = crate::stream::write_message(
            &mut send,
            &Message::Error {
                code: 1,
                message: "version mismatch".into(),
            },
        )
        .await;
        return Err(Error::VersionMismatch {
            local: inputsync_protocol::PROTOCOL_VERSION,
            remote: hello.protocol_version,
        });
    }

    let negotiated = Capabilities::full().negotiate(&hello.capabilities);
    let welcome = Message::Welcome(Welcome {
        peer_id: cfg.local_peer_id,
        peer_name: cfg.local_peer_name.clone(),
        accepted_capabilities: negotiated.clone(),
        assigned_screen: inputsync_core::ScreenId(1),
    });
    crate::stream::write_message(&mut send, &welcome).await?;

    let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(256);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Message>(256);

    let handle = PeerHandle::new(
        hello.peer_id,
        hello.peer_name.clone(),
        Some(remote),
        conn.clone(),
        outbound_tx,
    );

    let _ = events
        .send(ServerEvent::Connected {
            handle: handle.clone(),
            inbound: inbound_rx,
        })
        .await;

    crate::peer_loop::run_peer_loop(conn, send, recv, outbound_rx, inbound_tx).await;

    handle.clear().await;
    let _ = events
        .send(ServerEvent::Disconnected {
            peer_id: hello.peer_id,
        })
        .await;
    Ok(())
}

pub enum ServerEvent {
    Connected {
        handle: PeerHandle,
        inbound: mpsc::Receiver<Message>,
    },
    Disconnected {
        peer_id: PeerId,
    },
}
