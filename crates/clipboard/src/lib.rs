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
                // Read the image from the OS clipboard and re-encode as PNG
                // bytes for transfer. The write path decodes it back.
                let img = map_ab(c.get_image())?;
                let w = img.width as u32;
                let h = img.height as u32;
                let rgba = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())
                    .ok_or_else(|| Error::Other("clipboard image: bad dimensions".into()))?;
                let mut buf = Vec::with_capacity(8 * 1024);
                image::DynamicImage::ImageRgba8(rgba)
                    .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                    .map_err(|e| Error::Other(format!("encode png: {e}")))?;
                Ok(buf)
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
                // arboard expects raw RGBA pixel data + dimensions. Decode the
                // PNG bytes we received back into pixels.
                match image::load_from_memory_with_format(&payload.bytes, image::ImageFormat::Png) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        let w = rgba.width();
                        let h = rgba.height();
                        let img_data = arboard::ImageData {
                            width: w as usize,
                            height: h as usize,
                            bytes: std::borrow::Cow::Owned(rgba.into_raw()),
                        };
                        map_ab(c.set_image(img_data))?;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "clipboard write png: decode failed");
                    }
                }
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
        Ok(self
            .contents
            .lock()
            .get(&format)
            .cloned()
            .unwrap_or_default())
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
