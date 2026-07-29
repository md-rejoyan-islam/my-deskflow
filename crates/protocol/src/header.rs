use serde::{Deserialize, Serialize};

/// "ISYN" — magic bytes at the start of every frame.
pub const MAGIC: [u8; 4] = *b"ISYN";

/// Wire protocol version. Bumped on incompatible changes.
pub const PROTOCOL_VERSION: u16 = 1;

/// Fixed 12-byte frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub magic: [u8; 4],
    pub version: u16,
    pub message_type: u16,
    pub length: u32,
}

impl Header {
    pub const SIZE: usize = 12;

    pub fn new(message_type: MessageType, length: u32) -> Self {
        Self {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            message_type: message_type as u16,
            length,
        }
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version.to_be_bytes());
        out[6..8].copy_from_slice(&self.message_type.to_be_bytes());
        out[8..12].copy_from_slice(&self.length.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        Self {
            magic,
            version: u16::from_be_bytes([bytes[4], bytes[5]]),
            message_type: u16::from_be_bytes([bytes[6], bytes[7]]),
            length: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        }
    }
}

/// Message type identifiers. IDs are reserved even if the corresponding
/// feature is not implemented yet — this gives the protocol forward room.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    // Session / connection
    Hello = 0x0001,
    Welcome = 0x0002,
    Goodbye = 0x0003,

    // Input
    MouseMove = 0x0100,
    MouseButton = 0x0101,
    MouseScroll = 0x0102,
    KeyEvent = 0x0103,
    ScreenEnter = 0x0110,
    ScreenLeave = 0x0111,
    ModifierSync = 0x0112,

    // Clipboard
    ClipboardFormats = 0x0200,
    ClipboardRequest = 0x0201,
    ClipboardData = 0x0202,

    // File transfer
    FileOfferStart = 0x0300,
    FileChunk = 0x0301,
    FileAck = 0x0302,
    FileTransferCancel = 0x0303,

    // Control
    Ping = 0x0F00,
    Pong = 0x0F01,
    Error = 0x0F02,
}

impl MessageType {
    pub fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0x0001 => Self::Hello,
            0x0002 => Self::Welcome,
            0x0003 => Self::Goodbye,
            0x0100 => Self::MouseMove,
            0x0101 => Self::MouseButton,
            0x0102 => Self::MouseScroll,
            0x0103 => Self::KeyEvent,
            0x0110 => Self::ScreenEnter,
            0x0111 => Self::ScreenLeave,
            0x0112 => Self::ModifierSync,
            0x0200 => Self::ClipboardFormats,
            0x0201 => Self::ClipboardRequest,
            0x0202 => Self::ClipboardData,
            0x0300 => Self::FileOfferStart,
            0x0301 => Self::FileChunk,
            0x0302 => Self::FileAck,
            0x0303 => Self::FileTransferCancel,
            0x0F00 => Self::Ping,
            0x0F01 => Self::Pong,
            0x0F02 => Self::Error,
            _ => return None,
        })
    }
}
