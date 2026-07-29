//! Daemon-side clipboard glue.
//!
//! Polls the OS clipboard once per second. On change, hashes the content,
//! checks the originator registry to skip echoes from peers, and broadcasts
//! a `ClipboardFormats` advertisement over the active peer's outbound
//! channel. Actual data transfer is lazy — the peer requests bytes via
//! `ClipboardRequest`, and we respond by reading the OS clipboard right then.

use inputsync_clipboard::{default_backend, hash_content, ClipboardBackend, OriginatorRegistry};
use inputsync_core::PeerId;
use inputsync_network::PeerHandle;
use inputsync_protocol::{ClipboardFormat, ClipboardPayload, Message};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

pub struct ClipboardManager {
    backend: Arc<dyn ClipboardBackend>,
    originators: Arc<OriginatorRegistry>,
    last_hash: Arc<Mutex<Option<[u8; 32]>>>,
    local_peer_id: PeerId,
}

impl ClipboardManager {
    pub fn new(local_peer_id: PeerId) -> Self {
        Self {
            backend: default_backend(),
            originators: Arc::new(OriginatorRegistry::new()),
            last_hash: Arc::new(Mutex::new(None)),
            local_peer_id,
        }
    }

    pub fn backend(&self) -> Arc<dyn ClipboardBackend> {
        self.backend.clone()
    }

    /// Spawn a poll loop that broadcasts clipboard advertisements to all
    /// peers.
    pub fn spawn_poll_loop(&self, peers: Arc<Mutex<Vec<PeerHandle>>>) {
        let backend = self.backend.clone();
        let last_hash = self.last_hash.clone();
        let originators = self.originators.clone();
        let local_peer_id = self.local_peer_id;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1000));
            interval.tick().await;
            loop {
                interval.tick().await;
                // Only check text for v1 (PNG handling is incomplete).
                let bytes = match backend.read(ClipboardFormat::PlainText).await {
                    Ok(b) if !b.is_empty() => b,
                    _ => continue,
                };
                let hash = hash_content(&bytes);

                if last_hash.lock().as_ref() == Some(&hash) {
                    continue;
                }
                if originators.originator(&hash) == Some(local_peer_id) {
                    continue;
                }
                if originators.originator(&hash).is_some() {
                    // We already received this from a peer — don't echo back.
                    *last_hash.lock() = Some(hash);
                    continue;
                }
                *last_hash.lock() = Some(hash);
                originators.record(hash, local_peer_id);

                let active_peers = peers.lock().clone();
                for peer in active_peers {
                    let _ = peer
                        .send(Message::ClipboardFormats {
                            formats: vec![ClipboardFormat::PlainText],
                            hash,
                        })
                        .await;
                }
            }
        });
    }

    /// Handle an inbound clipboard message from a peer.
    pub async fn handle_inbound(&self, peer: &PeerHandle, msg: Message) {
        match msg {
            Message::ClipboardFormats { formats, hash } => {
                // Peer announced new clipboard. Auto-pull text.
                if formats.contains(&ClipboardFormat::PlainText) {
                    self.originators.record(hash, peer.peer_id);
                    let _ = peer
                        .send(Message::ClipboardRequest {
                            format: ClipboardFormat::PlainText,
                        })
                        .await;
                }
            }
            Message::ClipboardRequest { format } => {
                if let Ok(bytes) = self.backend.read(format).await {
                    let hash = hash_content(&bytes);
                    let _ = peer
                        .send(Message::ClipboardData(ClipboardPayload {
                            format,
                            bytes,
                            originator: self.local_peer_id,
                            hash,
                        }))
                        .await;
                }
            }
            Message::ClipboardData(payload) => {
                self.originators.record(payload.hash, payload.originator);
                *self.last_hash.lock() = Some(payload.hash);
                let _ = self.backend.write(payload).await;
            }
            _ => {}
        }
    }
}
