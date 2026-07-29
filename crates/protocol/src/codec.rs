use crate::header::{Header, MessageType, MAGIC, PROTOCOL_VERSION};
use crate::message::Message;
use bytes::{BufMut, BytesMut};
use thiserror::Error;

/// Hard cap on a single frame payload — 64 MB. Bulk file transfers chunk
/// into 64 KB and never approach this limit; the cap exists to bound
/// per-message allocations.
pub const MAX_PAYLOAD_SIZE: u32 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("incomplete frame: need {needed} more bytes")]
    Incomplete { needed: usize },

    #[error("bad magic bytes: {0:?}")]
    BadMagic([u8; 4]),

    #[error("unsupported protocol version: local={local} remote={remote}")]
    UnsupportedVersion { local: u16, remote: u16 },

    #[error("unknown message type id 0x{0:04x}")]
    UnknownMessageType(u16),

    #[error("payload too large: {0} > {max}", max = MAX_PAYLOAD_SIZE)]
    PayloadTooLarge(u32),

    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),
}

/// Serialize a [`Message`] into an owned byte vector, header included.
pub fn encode_frame(msg: &Message) -> Result<Vec<u8>, FrameError> {
    let payload = bincode::serialize(msg)?;
    if payload.len() as u64 > MAX_PAYLOAD_SIZE as u64 {
        return Err(FrameError::PayloadTooLarge(payload.len() as u32));
    }
    let header = Header::new(msg.message_type(), payload.len() as u32);

    let mut out = Vec::with_capacity(Header::SIZE + payload.len());
    out.extend_from_slice(&header.encode());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Append an encoded frame to a [`BytesMut`] without an extra allocation.
pub fn write_frame(buf: &mut BytesMut, msg: &Message) -> Result<(), FrameError> {
    let payload = bincode::serialize(msg)?;
    if payload.len() as u64 > MAX_PAYLOAD_SIZE as u64 {
        return Err(FrameError::PayloadTooLarge(payload.len() as u32));
    }
    let header = Header::new(msg.message_type(), payload.len() as u32);
    buf.reserve(Header::SIZE + payload.len());
    buf.put_slice(&header.encode());
    buf.put_slice(&payload);
    Ok(())
}

/// Try to read one frame from the head of `buf`.
///
/// On success, returns `Ok(Some((message, consumed_bytes)))`. The caller is
/// responsible for advancing `buf` by `consumed_bytes`.
///
/// On `Ok(None)`, the buffer holds an incomplete frame; caller should read
/// more bytes and retry.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(Message, usize)>, FrameError> {
    if buf.len() < Header::SIZE {
        return Ok(None);
    }
    let mut header_bytes = [0u8; Header::SIZE];
    header_bytes.copy_from_slice(&buf[..Header::SIZE]);
    let header = Header::decode(&header_bytes);

    if header.magic != MAGIC {
        return Err(FrameError::BadMagic(header.magic));
    }
    if header.version != PROTOCOL_VERSION {
        return Err(FrameError::UnsupportedVersion {
            local: PROTOCOL_VERSION,
            remote: header.version,
        });
    }
    if header.length > MAX_PAYLOAD_SIZE {
        return Err(FrameError::PayloadTooLarge(header.length));
    }

    let total = Header::SIZE + header.length as usize;
    if buf.len() < total {
        return Ok(None);
    }

    let payload = &buf[Header::SIZE..total];

    let _ty = MessageType::from_u16(header.message_type)
        .ok_or(FrameError::UnknownMessageType(header.message_type))?;
    let msg: Message = bincode::deserialize(payload)?;

    Ok(Some((msg, total)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use inputsync_core::PeerId;
    use crate::message::{Capabilities, Hello};

    #[test]
    fn roundtrip_hello() {
        let msg = Message::Hello(Hello {
            peer_id: PeerId::nil(),
            peer_name: "test".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: Capabilities::full(),
        });
        let bytes = encode_frame(&msg).unwrap();
        let (decoded, consumed) = decode_frame(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded.message_type(), msg.message_type());
    }

    #[test]
    fn incomplete_returns_none() {
        let msg = Message::Ping { nonce: 42, timestamp_ms: 0 };
        let bytes = encode_frame(&msg).unwrap();
        // Truncate header
        assert!(decode_frame(&bytes[..4]).unwrap().is_none());
        // Truncate payload
        assert!(decode_frame(&bytes[..bytes.len() - 1]).unwrap().is_none());
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = encode_frame(&Message::Ping { nonce: 1, timestamp_ms: 0 }).unwrap();
        bytes[0] = b'X';
        assert!(matches!(decode_frame(&bytes), Err(FrameError::BadMagic(_))));
    }
}
