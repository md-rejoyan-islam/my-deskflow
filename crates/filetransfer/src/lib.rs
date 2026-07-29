//! Chunked file transfer with blake3 verification and resume support.

use inputsync_core::{Error, Result};
use inputsync_protocol::{FileChunk, FileEntry, FileManifest};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Build a [`FileManifest`] for a single file. Multi-file manifests
/// concatenate these entries.
pub async fn build_entry(root: &Path, file: &Path) -> Result<FileEntry> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let mut f = File::open(&abs).await.map_err(Error::Io)?;
    let mut buf = vec![0u8; DEFAULT_CHUNK_SIZE];
    let mut hasher = blake3::Hasher::new();
    let mut total: u64 = 0;
    loop {
        let n = f.read(&mut buf).await.map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let relative = file.to_string_lossy().into_owned();
    Ok(FileEntry {
        relative_path: relative,
        size: total,
        blake3: *hasher.finalize().as_bytes(),
    })
}

pub async fn build_manifest(root: &Path, files: &[PathBuf]) -> Result<FileManifest> {
    let mut entries = Vec::with_capacity(files.len());
    let mut total = 0u64;
    for f in files {
        let entry = build_entry(root, f).await?;
        total += entry.size;
        entries.push(entry);
    }
    Ok(FileManifest {
        files: entries,
        total_bytes: total,
    })
}

/// Track in-progress sender state: which transfers are running, how many
/// chunks have been acknowledged.
#[derive(Default)]
pub struct SenderState {
    transfers: Mutex<HashMap<u64, SendProgress>>,
}

pub struct SendProgress {
    pub manifest: FileManifest,
    pub bytes_sent: u64,
    pub bytes_acked: u64,
}

impl SenderState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self, transfer_id: u64, manifest: FileManifest) {
        self.transfers.lock().insert(
            transfer_id,
            SendProgress {
                manifest,
                bytes_sent: 0,
                bytes_acked: 0,
            },
        );
    }

    pub fn record_sent(&self, transfer_id: u64, bytes: u64) {
        if let Some(p) = self.transfers.lock().get_mut(&transfer_id) {
            p.bytes_sent += bytes;
        }
    }

    pub fn record_ack(&self, transfer_id: u64, through: u64) {
        if let Some(p) = self.transfers.lock().get_mut(&transfer_id) {
            p.bytes_acked = through;
        }
    }
}

/// Track in-progress receiver state: which chunks have been written so that
/// the sender can resume after disconnect (summary §2.4 / §3.3 resume row).
pub struct Receiver {
    drop_dir: PathBuf,
    state: Arc<Mutex<RecvState>>,
}

#[derive(Default)]
struct RecvState {
    transfers: HashMap<u64, RecvProgress>,
}

struct RecvProgress {
    manifest: FileManifest,
    paths: Vec<PathBuf>,
    bytes_received: Vec<u64>,
    hashers: Vec<blake3::Hasher>,
}

impl Receiver {
    pub fn new(drop_dir: PathBuf) -> Self {
        Self {
            drop_dir,
            state: Arc::new(Mutex::new(RecvState::default())),
        }
    }

    pub async fn begin(&self, transfer_id: u64, manifest: FileManifest) -> Result<()> {
        tokio::fs::create_dir_all(&self.drop_dir).await.map_err(Error::Io)?;
        let mut paths = Vec::with_capacity(manifest.files.len());
        for e in &manifest.files {
            let sanitized = sanitize(&e.relative_path)?;
            let abs = self.drop_dir.join(&sanitized);
            if let Some(parent) = abs.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(Error::Io)?;
            }
            // Create or open for write.
            File::create(&abs).await.map_err(Error::Io)?;
            paths.push(abs);
        }
        let n = manifest.files.len();
        let mut hashers = Vec::with_capacity(n);
        for _ in 0..n {
            hashers.push(blake3::Hasher::new());
        }
        self.state.lock().transfers.insert(
            transfer_id,
            RecvProgress {
                manifest,
                paths,
                bytes_received: vec![0; n],
                hashers,
            },
        );
        Ok(())
    }

    pub async fn write_chunk(&self, chunk: FileChunk) -> Result<u64> {
        let (path, expected_offset, hasher_clone) = {
            let mut state = self.state.lock();
            let progress = state
                .transfers
                .get_mut(&chunk.transfer_id)
                .ok_or_else(|| Error::Other("unknown transfer".into()))?;
            let i = chunk.file_index as usize;
            if i >= progress.paths.len() {
                return Err(Error::Other("file index out of range".into()));
            }
            let path = progress.paths[i].clone();
            let expected = progress.bytes_received[i];
            (path, expected, progress.hashers[i].clone())
        };

        if chunk.offset != expected_offset {
            return Err(Error::Other(format!(
                "chunk out of order: expected offset {expected_offset}, got {}",
                chunk.offset
            )));
        }

        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(Error::Io)?;
        f.seek(std::io::SeekFrom::Start(chunk.offset)).await.map_err(Error::Io)?;
        f.write_all(&chunk.data).await.map_err(Error::Io)?;
        f.flush().await.map_err(Error::Io)?;

        let written = chunk.data.len() as u64;
        let mut new_hasher = hasher_clone;
        new_hasher.update(&chunk.data);

        let total_received = {
            let mut state = self.state.lock();
            let progress = state
                .transfers
                .get_mut(&chunk.transfer_id)
                .ok_or_else(|| Error::Other("unknown transfer".into()))?;
            let i = chunk.file_index as usize;
            progress.bytes_received[i] += written;
            progress.hashers[i] = new_hasher;

            if chunk.is_last {
                let expected_hash = progress.manifest.files[i].blake3;
                let got = *progress.hashers[i].clone().finalize().as_bytes();
                if got != expected_hash {
                    return Err(Error::Other(format!(
                        "blake3 mismatch on '{}'",
                        progress.manifest.files[i].relative_path
                    )));
                }
            }

            progress.bytes_received.iter().copied().sum::<u64>()
        };

        Ok(total_received)
    }

    /// Resume state lookup — sender calls this on reconnect to learn where
    /// to resume each file from.
    pub fn resume_offsets(&self, transfer_id: u64) -> Option<Vec<u64>> {
        self.state
            .lock()
            .transfers
            .get(&transfer_id)
            .map(|p| p.bytes_received.clone())
    }
}

/// Reject paths containing `..`, absolute components, or drive letters.
/// Prevents directory traversal attacks on the receiver side
/// (summary §4.6 path sanitization).
pub fn sanitize(rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    let p = PathBuf::from(&rel);
    if p.is_absolute() {
        return Err(Error::Other(format!("absolute path rejected: {rel}")));
    }
    for component in p.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => {
                return Err(Error::Other(format!(
                    "suspicious path component in: {rel}"
                )))
            }
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize("../etc/passwd").is_err());
        assert!(sanitize("/etc/passwd").is_err());
        assert!(sanitize("ok/file.txt").is_ok());
    }
}
