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

    pub async fn run(self, events: mpsc::Sender<ClientEvent>) -> Result<()> {
        let bind: SocketAddr = if self.cfg.server_addr.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        let mut endpoint =
            Endpoint::client(bind).map_err(|e| Error::Network(format!("client bind: {e}")))?;

        let client_tls = tls::client_rustls_config(Some(self.cfg.trusted_fingerprints.clone()))?;
        let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(client_tls)
            .map_err(|e| Error::Network(format!("quic client: {e}")))?;
        endpoint.set_default_client_config(QuinnClientConfig::new(Arc::new(qcc)));

        tracing::info!(addr = %self.cfg.server_addr, "connecting");
        let connection = endpoint
            .connect(self.cfg.server_addr, "inputsync.local")
            .map_err(|e| Error::Network(format!("connect: {e}")))?
            .await
            .map_err(|e| Error::Network(format!("handshake: {e}")))?;

        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| Error::Network(format!("open_bi: {e}")))?;

        let hello = Message::Hello(Hello {
            peer_id: self.cfg.local_peer_id,
            peer_name: self.cfg.local_peer_name.clone(),
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
