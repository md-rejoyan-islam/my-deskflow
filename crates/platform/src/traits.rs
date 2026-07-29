use crate::CursorPos;
use async_trait::async_trait;
use inputsync_core::{InputEvent, Result};
use std::sync::Arc;

/// Receiver-side of input events. Each backend pushes captured events into
/// an [`EventSink`] (typically an mpsc).
pub trait EventSink: Send + Sync {
    fn send(&self, event: InputEvent);
}

/// Capture local OS input events. All methods take `&self` — backends
/// internally manage hook lifecycle with atomics / locks.
#[async_trait]
pub trait Capture: Send + Sync {
    async fn start(&self, sink: Box<dyn EventSink>) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    fn set_capturing(&self, capturing: bool);
    fn cursor_position(&self) -> Result<CursorPos>;
    fn warp_cursor(&self, pos: CursorPos) -> Result<()>;
}

/// Inject input events from peers into the local OS.
#[async_trait]
pub trait Inject: Send + Sync {
    async fn inject(&self, event: InputEvent) -> Result<()>;
    async fn release_all_modifiers(&self) -> Result<()>;
}

/// A complete platform backend: cloneable Arc handles for both halves.
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;
    fn capture(&self) -> Arc<dyn Capture>;
    fn inject(&self) -> Arc<dyn Inject>;
}
