//! QUIC-based transport for InputSync.
//!
//! Streams (per summary §4.2):
//! - Bidi #0 — control (Hello / Welcome / Ping / Pong / Goodbye / Error)
//! - Uni  #1 — input events (priority)
//! - Bidi #2 — clipboard
//! - Bidi #3 — file transfer
//!
//! v1 wire connection uses a self-signed certificate with TOFU (trust-on-
//! first-use) and a pinned fingerprint stored in config. Production hardening
//! (proper CA chain, key rotation) is post-v1.0.

pub mod client;
pub mod peer;
pub mod peer_loop;
pub mod server;
pub mod stream;
pub mod tls;

pub use client::{
    connect_once, Client, ClientCommand, ClientConfig, ClientController, ClientEvent,
};
pub use peer::PeerHandle;
pub use server::{Server, ServerConfig, ServerEvent};
pub use stream::{read_message, write_message};
