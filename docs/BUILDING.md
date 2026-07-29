# Building InputSync

## Prerequisites

- Rust 1.75+ (toolchain pinned via `rust-toolchain.toml`)
- On Linux: `libudev-dev`, `libwayland-dev`, `libxkbcommon-dev`,
  `libgl1-mesa-dev` (for the GUI).
- On Windows: MSVC build tools (installed via VS Build Tools or
  `rustup default stable-msvc`).

## Build everything

```bash
cargo build --release
```

Outputs the three binaries to `target/release/`:

- `inputsync-daemon`
- `inputsync-cli`
- `inputsync-gui`

## Test

```bash
cargo test --workspace
```

## Run from source

Two-terminal local loopback test:

```bash
# Terminal 1 — server
cargo run --bin inputsync-daemon -- run --role server --listen 127.0.0.1:24800

# Terminal 2 — get the local fingerprint
cargo run --bin inputsync-cli -- fingerprint

# Terminal 3 — client (paste the fingerprint from step 2)
cargo run --bin inputsync-daemon -- run \
    --role client \
    --connect 127.0.0.1:24800 \
    --pin <fingerprint>
```

## Per-crate

```bash
cargo build -p inputsync-protocol
cargo test  -p inputsync-filetransfer
```
