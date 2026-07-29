//! Per-peer handle. Wraps a quinn::Connection's bidi control stream and
//! exposes async `send` for the daemon to push messages onto the wire.

use inputsync_core::{Error, PeerId, Result};
use inputsync_protocol::Message;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Handle held by the daemon for each connected peer. Cloneable so multiple
/// senders (input router, clipboard manager, file transfer task) can write
/// concurrently — the underlying mpsc is bounded for backpressure.
#[derive(Clone)]
pub struct PeerHandle {
    pub peer_id: PeerId,
    pub peer_name: String,
    pub remote_addr: Option<SocketAddr>,
    outbound: mpsc::Sender<Message>,
    /// Shared connection ref so the daemon can open auxiliary streams
    /// (file transfer, clipboard payloads) without going through the
    /// control channel.
    connection: Arc<Mutex<Option<quinn::Connection>>>,
}

impl PeerHandle {
    pub(crate) fn new(
        peer_id: PeerId,
        peer_name: String,
        remote_addr: Option<SocketAddr>,
        connection: quinn::Connection,
        outbound: mpsc::Sender<Message>,
    ) -> Self {
        Self {
            peer_id,
            peer_name,
            remote_addr,
            outbound,
            connection: Arc::new(Mutex::new(Some(connection))),
        }
    }

    pub async fn send(&self, msg: Message) -> Result<()> {
        self.outbound
            .send(msg)
            .await
            .map_err(|_| Error::Network("peer outbound channel closed".into()))
    }

    pub fn try_send(&self, msg: Message) -> Result<()> {
        self.outbound
            .try_send(msg)
            .map_err(|e| Error::Network(format!("peer outbound: {e}")))
    }

    /// Open an auxiliary bidirectional stream — used for clipboard payload
    /// fetches and file transfers so bulk traffic doesn't share the
    /// input-priority control stream.
    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream)> {
        let conn = self.connection.lock().await;
        let conn = conn
            .as_ref()
            .ok_or_else(|| Error::Network("peer disconnected".into()))?;
        conn.open_bi()
            .await
            .map_err(|e| Error::Network(format!("open_bi: {e}")))
    }

    pub(crate) async fn clear(&self) {
        *self.connection.lock().await = None;
    }
}
