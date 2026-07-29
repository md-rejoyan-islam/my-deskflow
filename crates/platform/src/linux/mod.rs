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
    Ok(Box::new(LinuxPlatform {
        capture: Arc::new(LinuxCapture::new()),
        inject: Arc::new(LinuxInject::new()?),
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
    inject: Arc<LinuxInject>,
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
