//! Linux X11 injection stub. Will use XTEST extension on X11 and uinput on
//! Wayland. Currently a no-op returning errors so the rest of the daemon
//! compiles on Linux.

use crate::traits::Inject;
use async_trait::async_trait;
use inputsync_core::{Error, InputEvent, Result};

pub struct LinuxInject {
    _private: (),
}

impl LinuxInject {
    pub fn new() -> Result<Self> {
        Ok(Self { _private: () })
    }
}

#[async_trait]
impl Inject for LinuxInject {
    async fn inject(&self, _event: InputEvent) -> Result<()> {
        Err(Error::Platform("linux inject not yet implemented".into()))
    }
    async fn release_all_modifiers(&self) -> Result<()> {
        Ok(())
    }
}
