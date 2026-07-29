//! Heartbeat task — periodically sends Ping. The peer's inbound_loop
//! responds with Pong. We don't currently terminate on missing Pong (the
//! QUIC keep-alive provides a similar safety net), but the task is the
//! place where that logic will live.

use inputsync_network::PeerHandle;
use inputsync_protocol::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn spawn_heartbeat(peer: PeerHandle, interval_ms: u64) {
    tokio::spawn(async move {
        let mut nonce: u64 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(500)));
        interval.tick().await;
        loop {
            interval.tick().await;
            nonce = nonce.wrapping_add(1);
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if peer
                .send(Message::Ping {
                    nonce,
                    timestamp_ms: ts,
                })
                .await
                .is_err()
            {
                tracing::debug!(peer = %peer.peer_id, "heartbeat: peer outbound closed");
                return;
            }
        }
    });
}
