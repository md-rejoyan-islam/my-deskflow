//! InputSync wire protocol.
//!
//! ## Frame layout
//! Every frame is a fixed 12-byte header followed by a bincode-serialized
//! payload of `length` bytes.
//!
//! ```text
//! ┌────────────┬─────────┬──────────┬─────────────┐
//! │ magic (4)  │ ver (2) │ type (2) │ length (4)  │
//! │  "ISYN"    │  u16    │   u16    │     u32     │
//! └────────────┴─────────┴──────────┴─────────────┘
//! ```

pub mod codec;
pub mod header;
pub mod message;

pub use codec::{decode_frame, encode_frame, FrameError, MAX_PAYLOAD_SIZE};
pub use header::{Header, MessageType, MAGIC, PROTOCOL_VERSION};
pub use message::{
    Capabilities, ClipboardFormat, ClipboardPayload, FileChunk, FileEntry, FileManifest, FileOffer,
    Goodbye, Hello, Message, Welcome,
};
