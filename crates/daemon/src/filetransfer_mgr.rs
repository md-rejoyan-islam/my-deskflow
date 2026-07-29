//! File transfer manager — opens a fresh QUIC bidi stream per transfer
//! and streams `FileChunk` messages until the manifest is exhausted.
//!
//! The receiver listens on the control channel for `FileOfferStart`,
//! prepares its drop dir via `Receiver::begin`, then accepts the bidi
//! stream the sender opens and writes chunks via `Receiver::write_chunk`.

use anyhow::{Context, Result};
use inputsync_filetransfer::{build_manifest, Receiver, DEFAULT_CHUNK_SIZE};
use inputsync_network::{stream::write_message, PeerHandle};
use inputsync_protocol::{FileChunk, FileOffer, Message};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static TRANSFER_ID_SEQ: AtomicU64 = AtomicU64::new(1);

pub struct FileTransferManager {
    pub drop_dir: PathBuf,
    pub receiver: Arc<Receiver>,
}

impl FileTransferManager {
    pub fn new(drop_dir: PathBuf) -> Self {
        let receiver = Arc::new(Receiver::new(drop_dir.clone()));
        Self { drop_dir, receiver }
    }

    /// Send a single file (or set of files relative to `root`) to a peer.
    pub async fn send_files(
        &self,
        peer: &PeerHandle,
        root: &Path,
        files: Vec<PathBuf>,
        compress: bool,
    ) -> Result<u64> {
        let manifest = build_manifest(root, &files).await.context("build manifest")?;
        let transfer_id = TRANSFER_ID_SEQ.fetch_add(1, Ordering::SeqCst);

        // Announce on the control channel.
        peer.send(Message::FileOfferStart(FileOffer {
            transfer_id,
            manifest: manifest.clone(),
            compressed: compress,
        }))
        .await?;

        // Open a fresh bidi stream for chunks.
        let (mut send, _recv) = peer.open_bi().await.context("open file stream")?;

        for (file_index, entry) in manifest.files.iter().enumerate() {
            let abs = if Path::new(&entry.relative_path).is_absolute() {
                PathBuf::from(&entry.relative_path)
            } else {
                root.join(&entry.relative_path)
            };
            let mut file = tokio::fs::File::open(&abs)
                .await
                .with_context(|| format!("open {}", abs.display()))?;
            let mut buf = vec![0u8; DEFAULT_CHUNK_SIZE];
            let mut offset: u64 = 0;
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf)
                    .await
                    .context("read chunk")?;
                let is_last = n < buf.len();
                if n > 0 {
                    let chunk = FileChunk {
                        transfer_id,
                        file_index: file_index as u32,
                        offset,
                        data: buf[..n].to_vec(),
                        is_last,
                    };
                    write_message(&mut send, &Message::FileChunk(chunk))
                        .await
                        .context("write chunk")?;
                    offset += n as u64;
                }
                if is_last || n == 0 {
                    break;
                }
            }
        }
        send.finish().ok();
        Ok(transfer_id)
    }

    pub async fn handle_offer(&self, offer: FileOffer) -> Result<()> {
        self.receiver
            .begin(offer.transfer_id, offer.manifest)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }

    pub async fn handle_chunk(&self, chunk: FileChunk) -> Result<u64> {
        self.receiver
            .write_chunk(chunk)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}
