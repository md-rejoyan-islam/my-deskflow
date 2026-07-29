//! Platform abstraction layer.
//!
//! Each supported OS implements [`Capture`] (for grabbing local input
//! events) and [`Inject`] (for replaying remote events into the local OS).
//! The daemon never references OS APIs directly — it goes through these
//! traits.

use inputsync_core::{InputEvent, Result, ScreenInfo};

pub mod traits;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows as backend;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux as backend;

#[cfg(not(any(windows, target_os = "linux")))]
pub mod stub;
#[cfg(not(any(windows, target_os = "linux")))]
pub use stub as backend;

pub use traits::{Capture, EventSink, Inject, Platform};

/// Construct the best platform backend for the current OS.
pub fn current() -> Result<Box<dyn Platform>> {
    backend::new()
}

/// Convenience: enumerate physical screens on this machine and produce a
/// single logical [`ScreenInfo`].
pub fn local_screen_info() -> Result<ScreenInfo> {
    backend::local_screen_info()
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    backend::enumerate_monitors()
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

/// A snapshot of the local cursor position (in virtual desktop coordinates).
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorPos {
    pub x: i32,
    pub y: i32,
}

/// Generic helper used by capture backends: drop into a tight callback that
/// only forwards raw events to a channel. This is the "hook must never
/// block" guarantee in code form.
pub fn forward<T: Send + 'static>(
    tx: crossbeam_channel::Sender<T>,
    item: T,
) -> std::result::Result<(), crossbeam_channel::TrySendError<T>> {
    tx.try_send(item)
}

#[allow(dead_code)]
fn _assert_input_event_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<InputEvent>();
    assert_sync::<InputEvent>();
}
