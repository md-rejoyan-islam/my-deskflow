//! Clipboard manager. Implements the lazy-clipboard model (summary §2.4).

use async_trait::async_trait;
use inputsync_core::{Error, PeerId, Result};
use inputsync_protocol::{ClipboardFormat, ClipboardPayload};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

#[async_trait]
pub trait ClipboardBackend: Send + Sync {
    async fn list_formats(&self) -> Result<Vec<ClipboardFormat>>;
    async fn read(&self, format: ClipboardFormat) -> Result<Vec<u8>>;
    async fn write(&self, payload: ClipboardPayload) -> Result<()>;
    async fn is_confidential(&self) -> Result<bool> {
        Ok(false)
    }
}

#[derive(Default)]
pub struct OriginatorRegistry {
    seen: Mutex<HashMap<[u8; 32], PeerId>>,
}

impl OriginatorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, hash: [u8; 32], originator: PeerId) {
        self.seen.lock().insert(hash, originator);
    }

    pub fn originator(&self, hash: &[u8; 32]) -> Option<PeerId> {
        self.seen.lock().get(hash).copied()
    }
}

pub fn hash_content(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Construct the OS-default backend. Uses `arboard` which supports
/// Windows, X11, Wayland, and macOS.
pub fn default_backend() -> Arc<dyn ClipboardBackend> {
    Arc::new(ArboardBackend::new())
}

fn map_ab<T>(r: std::result::Result<T, arboard::Error>) -> Result<T> {
    r.map_err(|e| Error::Other(format!("clipboard: {e}")))
}

pub struct ArboardBackend {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl ArboardBackend {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn ensure(&self) -> Result<()> {
        let mut guard = self.inner.lock();
        if guard.is_none() {
            *guard = Some(
                arboard::Clipboard::new()
                    .map_err(|e| Error::Other(format!("arboard init: {e}")))?,
            );
        }
        Ok(())
    }
}

#[async_trait]
impl ClipboardBackend for ArboardBackend {
    async fn list_formats(&self) -> Result<Vec<ClipboardFormat>> {
        self.ensure()?;
        let mut guard = self.inner.lock();
        let c = guard.as_mut().unwrap();
        let mut formats = Vec::new();
        if matches!(c.get_text(), Ok(s) if !s.is_empty()) {
            formats.push(ClipboardFormat::PlainText);
        }
        if c.get_image().is_ok() {
            formats.push(ClipboardFormat::Png);
        }
        Ok(formats)
    }

    async fn read(&self, format: ClipboardFormat) -> Result<Vec<u8>> {
        self.ensure()?;
        let mut guard = self.inner.lock();
        let c = guard.as_mut().unwrap();
        match format {
            ClipboardFormat::PlainText => {
                let text = map_ab(c.get_text())?;
                Ok(text.into_bytes())
            }
            ClipboardFormat::Png => {
                let img = map_ab(c.get_image())?;
                Ok(img.bytes.into_owned())
            }
            ClipboardFormat::Html | ClipboardFormat::Rtf | ClipboardFormat::UriList => Ok(vec![]),
        }
    }

    async fn write(&self, payload: ClipboardPayload) -> Result<()> {
        self.ensure()?;
        let mut guard = self.inner.lock();
        let c = guard.as_mut().unwrap();
        match payload.format {
            ClipboardFormat::PlainText => {
                let text = String::from_utf8(payload.bytes)
                    .map_err(|e| Error::Other(format!("clipboard utf8: {e}")))?;
                map_ab(c.set_text(text))?;
            }
            ClipboardFormat::Png => {
                tracing::debug!("clipboard write png: not yet wired");
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct StubBackend {
    formats: Mutex<Vec<ClipboardFormat>>,
    contents: Mutex<HashMap<ClipboardFormat, Vec<u8>>>,
}

#[async_trait]
impl ClipboardBackend for StubBackend {
    async fn list_formats(&self) -> Result<Vec<ClipboardFormat>> {
        Ok(self.formats.lock().clone())
    }
    async fn read(&self, format: ClipboardFormat) -> Result<Vec<u8>> {
        Ok(self.contents.lock().get(&format).cloned().unwrap_or_default())
    }
    async fn write(&self, payload: ClipboardPayload) -> Result<()> {
        let mut formats = self.formats.lock();
        if !formats.contains(&payload.format) {
            formats.push(payload.format);
        }
        self.contents.lock().insert(payload.format, payload.bytes);
        Ok(())
    }
}
