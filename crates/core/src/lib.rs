//! Shared types and configuration for InputSync.
//!
//! This crate has zero OS dependencies and is consumed by every other crate
//! in the workspace.

pub mod config;
pub mod error;
pub mod event;
pub mod id;
pub mod screen;

pub use config::{ClipboardConfig, Config, FileTransferConfig, NetworkConfig, ServerRole};
pub use error::{Error, Result};
pub use event::{
    Button, InputEvent, KeyCode, KeyEvent, KeyState, ModifierState, MouseEvent, ScrollDelta,
};
pub use id::{ClientId, PeerId, ScreenId};
pub use screen::{EdgeSide, Point, ScreenInfo, ScreenLayout};
