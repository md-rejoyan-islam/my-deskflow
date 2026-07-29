//! Helpers for reading and writing protocol [`Message`]s on a quinn stream.

use inputsync_core::{Error, Result};
use inputsync_protocol::{decode_frame, encode_frame, Header, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn write_message<W: AsyncWriteExt + Unpin>(w: &mut W, msg: &Message) -> Result<()> {
    let bytes = encode_frame(msg).map_err(|e| Error::Protocol(format!("encode: {e}")))?;
    w.write_all(&bytes)
        .await
        .map_err(|e| Error::Network(format!("write: {e}")))?;
    Ok(())
}

pub async fn read_message<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Message> {
    let mut header_buf = [0u8; Header::SIZE];
    r.read_exact(&mut header_buf)
        .await
        .map_err(|e| Error::Network(format!("read header: {e}")))?;
    let header = Header::decode(&header_buf);

    let mut payload = vec![0u8; header.length as usize];
    if !payload.is_empty() {
        r.read_exact(&mut payload)
            .await
            .map_err(|e| Error::Network(format!("read payload: {e}")))?;
    }

    let mut framed = Vec::with_capacity(Header::SIZE + payload.len());
    framed.extend_from_slice(&header_buf);
    framed.extend_from_slice(&payload);

    let (msg, _consumed) = decode_frame(&framed)
        .map_err(|e| Error::Protocol(format!("decode: {e}")))?
        .ok_or_else(|| Error::Protocol("decode produced no message".into()))?;
    Ok(msg)
}
