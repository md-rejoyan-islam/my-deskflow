//! Fallback backend for unsupported platforms.

use crate::traits::{Capture, EventSink, Inject, Platform};
use crate::{CursorPos, MonitorInfo};
use async_trait::async_trait;
use inputsync_core::{Error, InputEvent, Result, ScreenId, ScreenInfo};
use std::sync::Arc;

pub fn new() -> Result<Box<dyn Platform>> {
    Err(Error::Platform(format!(
        "no platform backend available for {}",
        std::env::consts::OS
    )))
}

pub fn local_screen_info() -> Result<ScreenInfo> {
    Ok(ScreenInfo {
        id: ScreenId(0),
        name: "stub".into(),
        width: 0,
        height: 0,
    })
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    Ok(vec![])
}

pub struct StubPlatform {
    capture: Arc<StubCapture>,
    inject: Arc<StubInject>,
}

impl StubPlatform {
    pub fn new() -> Self {
        Self {
            capture: Arc::new(StubCapture),
            inject: Arc::new(StubInject),
        }
    }
}

impl Platform for StubPlatform {
    fn name(&self) -> &'static str {
        "stub"
    }
    fn capture(&self) -> Arc<dyn Capture> {
        self.capture.clone()
    }
    fn inject(&self) -> Arc<dyn Inject> {
        self.inject.clone()
    }
}

pub struct StubCapture;

#[async_trait]
impl Capture for StubCapture {
    async fn start(&self, _sink: Box<dyn EventSink>) -> Result<()> {
        Err(Error::Platform("stub: capture unsupported".into()))
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn set_capturing(&self, _capturing: bool) {}
    fn cursor_position(&self) -> Result<CursorPos> {
        Ok(CursorPos::default())
    }
    fn warp_cursor(&self, _pos: CursorPos) -> Result<()> {
        Ok(())
    }
}

pub struct StubInject;

#[async_trait]
impl Inject for StubInject {
    async fn inject(&self, _event: InputEvent) -> Result<()> {
        Err(Error::Platform("stub: inject unsupported".into()))
    }
    async fn release_all_modifiers(&self) -> Result<()> {
        Ok(())
    }
}
