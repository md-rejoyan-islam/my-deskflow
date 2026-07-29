//! Linux platform backend.

mod inject;
mod monitor;

use crate::traits::{Capture, EventSink, Inject, Platform};
use crate::{CursorPos, MonitorInfo};
use async_trait::async_trait;
use inputsync_core::{Error, InputEvent, Result, ScreenId, ScreenInfo};
use std::sync::Arc;

pub use inject::LinuxInject;

pub fn new() -> Result<Box<dyn Platform>> {
    // The inject backend opens /dev/uinput, which can fail if the user isn't
    // in the `uinput` group (haven't logged out/in yet) or the module isn't
    // loaded. We must NOT crash the daemon over this — the daemon needs to
    // stay alive so the GUI can show status. Fall back to a no-op inject and
    // log loudly; capture is already a stub on Linux.
    let inject: Arc<dyn Inject> = match LinuxInject::new() {
        Ok(i) => Arc::new(i),
        Err(e) => {
            tracing::error!(
                error = %e,
                "failed to initialize /dev/uinput injection; \
                 input injection will not work until you log out/in \
                 (for the uinput group) or load the uinput kernel module. \
                 The daemon will keep running so the GUI stays reachable."
            );
            Arc::new(NullInject)
        }
    };
    Ok(Box::new(LinuxPlatform {
        capture: Arc::new(LinuxCapture::new()),
        inject,
    }))
}

pub fn local_screen_info() -> Result<ScreenInfo> {
    let monitors = monitor::enumerate()?;
    let (min_x, min_y, max_x, max_y) = monitors.iter().fold(
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        |(mnx, mny, mxx, mxy), m| {
            (
                mnx.min(m.x),
                mny.min(m.y),
                mxx.max(m.x + m.width),
                mxy.max(m.y + m.height),
            )
        },
    );
    let (width, height) = if monitors.is_empty() {
        (0, 0)
    } else {
        ((max_x - min_x).max(0), (max_y - min_y).max(0))
    };
    Ok(ScreenInfo {
        id: ScreenId(0),
        name: hostname(),
        width,
        height,
    })
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    monitor::enumerate()
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "linux".into())
}

pub struct LinuxPlatform {
    capture: Arc<LinuxCapture>,
    inject: Arc<dyn Inject>,
}

impl Platform for LinuxPlatform {
    fn name(&self) -> &'static str {
        "linux-x11"
    }
    fn capture(&self) -> Arc<dyn Capture> {
        self.capture.clone()
    }
    fn inject(&self) -> Arc<dyn Inject> {
        self.inject.clone()
    }
}

/// No-op inject backend used as a fallback when `/dev/uinput` can't be opened
/// (e.g. user hasn't logged out/in for the `uinput` group yet). Keeps the
/// daemon alive so the GUI stays reachable; injection silently does nothing.
struct NullInject;

#[async_trait]
impl Inject for NullInject {
    async fn inject(&self, _event: InputEvent) -> Result<()> {
        // Silently drop — logged once at startup.
        Ok(())
    }
    async fn release_all_modifiers(&self) -> Result<()> {
        Ok(())
    }
}

pub struct LinuxCapture {
    capturing: parking_lot::Mutex<bool>,
}

impl LinuxCapture {
    pub fn new() -> Self {
        Self {
            capturing: parking_lot::Mutex::new(false),
        }
    }
}

#[async_trait]
impl Capture for LinuxCapture {
    async fn start(&self, _sink: Box<dyn EventSink>) -> Result<()> {
        tracing::warn!("linux capture: stub — no events will be produced yet");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        *self.capturing.lock() = false;
        Ok(())
    }

    fn set_capturing(&self, capturing: bool) {
        *self.capturing.lock() = capturing;
    }

    fn cursor_position(&self) -> Result<CursorPos> {
        Err(Error::Platform(
            "linux cursor query not yet implemented".into(),
        ))
    }

    fn warp_cursor(&self, _pos: CursorPos) -> Result<()> {
        Err(Error::Platform(
            "linux warp_cursor not yet implemented".into(),
        ))
    }
}

#[allow(dead_code)]
fn _unused(_e: InputEvent) {}
