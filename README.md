# InputSync

A cross-platform keyboard, mouse, clipboard, and file-sharing tool for controlling multiple computers from a single workstation. Clean-room rewrite of Synergy / Barrier / Deskflow in Rust.

See [summary.md](summary.md) for the full project overview.

## Quick start

```bash
# Build everything
cargo build --release

# Run the daemon as the "server" on machine A
cargo run --release --bin inputsync-daemon -- run --role server --listen 0.0.0.0:24800

# Run the daemon as the "client" on machine B
cargo run --release --bin inputsync-daemon -- run --role client --connect 192.168.1.10:24800

# Talk to the running daemon
cargo run --release --bin inputsync-cli -- status
cargo run --release --bin inputsync-cli -- watch

# Launch the GUI
cargo run --release --bin inputsync-gui
```

## Workspace layout

| Crate | Purpose |
|---|---|
| `core` | Shared types (events, ids, errors, config). No OS deps. |
| `protocol` | Wire format, framed codec, bincode messages. |
| `platform` | OS abstraction trait + Windows + Linux backends. |
| `network` | QUIC transport (client + server). |
| `clipboard` | Lazy clipboard sync. |
| `filetransfer` | Chunked file transfer with resume. |
| `ipc` | Daemon ↔ GUI/CLI socket protocol. |
| `daemon` | Service binary. |
| `cli` | Headless control tool. |
| `gui` | egui application. |

## Status

Foundation laid. v1.0 roadmap in [summary.md §7](summary.md).
