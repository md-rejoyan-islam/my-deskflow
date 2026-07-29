# InputSync wire protocol

Authoritative description of the binary protocol carried over QUIC streams
between paired InputSync machines. See `crates/protocol/` for the
implementation; both must stay in sync.

## Framing

Every frame is a fixed 12-byte header followed by a `length`-byte
bincode-serialized payload.

```
┌────────────┬─────────┬──────────┬─────────────┐
│ magic (4)  │ ver (2) │ type (2) │ length (4)  │
│  "ISYN"    │  u16    │   u16    │     u32     │
└────────────┴─────────┴──────────┴─────────────┘
```

- All integers are big-endian.
- `magic` is the ASCII bytes `0x49 0x53 0x59 0x4E` ("ISYN").
- `version` starts at 1; bumped on any breaking change.
- `length` is the payload size in bytes (max 64 MiB).

## Message type IDs

| ID | Name | Direction | Stream |
|---|---|---|---|
| 0x0001 | Hello | client → server | control (bidi #0) |
| 0x0002 | Welcome | server → client | control |
| 0x0003 | Goodbye | either | control |
| 0x0100 | MouseMove | server → client | input (uni) |
| 0x0101 | MouseButton | server → client | input |
| 0x0102 | MouseScroll | server → client | input |
| 0x0103 | KeyEvent | server → client | input |
| 0x0110 | ScreenEnter | server → client | input |
| 0x0111 | ScreenLeave | server → client | input |
| 0x0112 | ModifierSync | server → client | input |
| 0x0200 | ClipboardFormats | either | clipboard (bidi) |
| 0x0201 | ClipboardRequest | either | clipboard |
| 0x0202 | ClipboardData | either | clipboard |
| 0x0300 | FileOfferStart | sender → receiver | filetransfer (bidi) |
| 0x0301 | FileChunk | sender → receiver | filetransfer |
| 0x0302 | FileAck | receiver → sender | filetransfer |
| 0x0303 | FileTransferCancel | either | filetransfer |
| 0x0F00 | Ping | either | control |
| 0x0F01 | Pong | either | control |
| 0x0F02 | Error | either | control |

## Handshake

```
Client (B)                             Server (A)
  │ ─── QUIC connect (TLS 1.3) ──────► │
  │ ◄── TLS handshake complete ─────── │
  │ ─── Hello {peer_id, version, caps} ──► │
  │                              verify pin
  │                              check version
  │                              negotiate caps
  │ ◄── Welcome {peer_id, accepted_caps} ── │
  │ ═══ Ping/Pong every 2 s ════════════════ │
```

## Reconnect

On any I/O error, the client resets to an initial backoff (default 500 ms),
re-runs the handshake on a fresh QUIC connection, and resumes. File
transfers re-negotiate by file-index and offset via `FileOfferStart` +
`FileAck`.

## Heartbeat

Both sides send `Ping { nonce, timestamp_ms }` every
`heartbeat_interval_ms` (default 2000). A missed `Pong` for
`heartbeat_timeout_ms` (default 6000) terminates the connection and triggers
reconnect.
