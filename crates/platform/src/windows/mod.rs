//! Windows platform backend.

mod capture;
mod inject;
mod keymap;
mod monitor;

pub use capture::WindowsCapture;
pub use inject::WindowsInject;

use crate::traits::{Capture, Inject, Platform};
use crate::MonitorInfo;
use inputsync_core::{Result, ScreenId, ScreenInfo};
use std::sync::Arc;

pub fn new() -> Result<Box<dyn Platform>> {
    Ok(Box::new(WindowsPlatform {
        capture: Arc::new(WindowsCapture::new()),
        inject: Arc::new(WindowsInject::new()),
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
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows".into())
}

pub struct WindowsPlatform {
    capture: Arc<WindowsCapture>,
    inject: Arc<WindowsInject>,
}

impl Platform for WindowsPlatform {
    fn name(&self) -> &'static str {
        "windows"
    }
    fn capture(&self) -> Arc<dyn Capture> {
        self.capture.clone()
    }
    fn inject(&self) -> Arc<dyn Inject> {
        self.inject.clone()
    }
}
