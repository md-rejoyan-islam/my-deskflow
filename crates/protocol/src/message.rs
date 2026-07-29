use crate::header::MessageType;
use inputsync_core::{InputEvent, KeyEvent, ModifierState, MouseEvent, PeerId, ScreenId};
use serde::{Deserialize, Serialize};

/// All payloads that can be carried on the wire. Each variant maps 1:1 to a
/// [`MessageType`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    Hello(Hello),
    Welcome(Welcome),
    Goodbye(Goodbye),

    MouseMove {
        x: i32,
        y: i32,
    },
    /// Relative mouse movement delta (used when the server's cursor is
    /// pinned at a screen edge after a crossing — the client applies dx/dy).
    MouseMoveRelative {
        dx: i32,
        dy: i32,
    },
    MouseButton(MouseEvent),
    MouseScroll(MouseEvent),
    KeyEvent(KeyEvent),
    ScreenEnter {
        x: i32,
        y: i32,
        modifiers: ModifierState,
    },
    ScreenLeave {
        peer_screen: u32,
    },
    ModifierSync(ModifierState),

    ClipboardFormats {
        formats: Vec<ClipboardFormat>,
        hash: [u8; 32],
    },
    ClipboardRequest {
        format: ClipboardFormat,
    },
    ClipboardData(ClipboardPayload),

    FileOfferStart(FileOffer),
    FileChunk(FileChunk),
    FileAck {
        transfer_id: u64,
        received_through: u64,
    },
    FileTransferCancel {
        transfer_id: u64,
        reason: String,
    },

    Ping {
        nonce: u64,
        timestamp_ms: u64,
    },
    Pong {
        nonce: u64,
        timestamp_ms: u64,
    },
    Error {
        code: u32,
        message: String,
    },
}

impl Message {
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::Hello(_) => MessageType::Hello,
            Self::Welcome(_) => MessageType::Welcome,
            Self::Goodbye(_) => MessageType::Goodbye,
            Self::MouseMove { .. } => MessageType::MouseMove,
            Self::MouseMoveRelative { .. } => MessageType::MouseMoveRelative,
            Self::MouseButton(_) => MessageType::MouseButton,
            Self::MouseScroll(_) => MessageType::MouseScroll,
            Self::KeyEvent(_) => MessageType::KeyEvent,
            Self::ScreenEnter { .. } => MessageType::ScreenEnter,
            Self::ScreenLeave { .. } => MessageType::ScreenLeave,
            Self::ModifierSync(_) => MessageType::ModifierSync,
            Self::ClipboardFormats { .. } => MessageType::ClipboardFormats,
            Self::ClipboardRequest { .. } => MessageType::ClipboardRequest,
            Self::ClipboardData(_) => MessageType::ClipboardData,
            Self::FileOfferStart(_) => MessageType::FileOfferStart,
            Self::FileChunk(_) => MessageType::FileChunk,
            Self::FileAck { .. } => MessageType::FileAck,
            Self::FileTransferCancel { .. } => MessageType::FileTransferCancel,
            Self::Ping { .. } => MessageType::Ping,
            Self::Pong { .. } => MessageType::Pong,
            Self::Error { .. } => MessageType::Error,
        }
    }

    /// Translate from a high-level [`InputEvent`] to a wire [`Message`].
    pub fn from_input(ev: InputEvent) -> Self {
        match ev {
            InputEvent::Mouse(MouseEvent::Move { x, y }) => Self::MouseMove { x, y },
            InputEvent::Mouse(MouseEvent::MoveRelative { dx, dy }) => {
                Self::MouseMoveRelative { dx, dy }
            }
            InputEvent::Mouse(e @ MouseEvent::Button { .. }) => Self::MouseButton(e),
            InputEvent::Mouse(e @ MouseEvent::Scroll(_)) => Self::MouseScroll(e),
            InputEvent::Key(k) => Self::KeyEvent(k),
            InputEvent::ScreenEnter { x, y, modifiers } => Self::ScreenEnter { x, y, modifiers },
            InputEvent::ScreenLeave { peer_screen } => Self::ScreenLeave { peer_screen },
            InputEvent::ModifierSync(m) => Self::ModifierSync(m),
        }
    }
}

/// First message sent by a connecting client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub peer_id: PeerId,
    pub peer_name: String,
    pub protocol_version: u16,
    pub capabilities: Capabilities,
}

/// Server's accept message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub peer_id: PeerId,
    pub peer_name: String,
    pub accepted_capabilities: Capabilities,
    pub assigned_screen: ScreenId,
}

/// Clean shutdown notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goodbye {
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub clipboard_text: bool,
    pub clipboard_html: bool,
    pub clipboard_image: bool,
    pub clipboard_files: bool,
    pub file_transfer: bool,
    pub zstd_compression: bool,
}

impl Capabilities {
    pub fn full() -> Self {
        Self {
            clipboard_text: true,
            clipboard_html: true,
            clipboard_image: true,
            clipboard_files: true,
            file_transfer: true,
            zstd_compression: true,
        }
    }

    /// Intersection — only features both sides support.
    pub fn negotiate(&self, other: &Self) -> Self {
        Self {
            clipboard_text: self.clipboard_text && other.clipboard_text,
            clipboard_html: self.clipboard_html && other.clipboard_html,
            clipboard_image: self.clipboard_image && other.clipboard_image,
            clipboard_files: self.clipboard_files && other.clipboard_files,
            file_transfer: self.file_transfer && other.file_transfer,
            zstd_compression: self.zstd_compression && other.zstd_compression,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipboardFormat {
    PlainText,
    Html,
    Rtf,
    Png,
    UriList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPayload {
    pub format: ClipboardFormat,
    pub bytes: Vec<u8>,
    pub originator: PeerId,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOffer {
    pub transfer_id: u64,
    pub manifest: FileManifest,
    pub compressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub files: Vec<FileEntry>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub relative_path: String,
    pub size: u64,
    pub blake3: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub transfer_id: u64,
    pub file_index: u32,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
}
