\# InputSync — Project Overview



> A cross-platform keyboard, mouse, clipboard, and file-sharing tool for controlling multiple computers from a single workstation. Built from scratch in Rust for Windows and Linux.



\---



\## 1. Project Summary



InputSync lets you use one keyboard and mouse to control multiple computers placed side-by-side. Move your cursor off the edge of one screen, and it appears on the next machine — keyboard input follows automatically. Clipboard contents and files sync between machines transparently.



This project exists because the current options in this space — \*\*Synergy\*\*, \*\*Barrier\*\*, \*\*InputLeap\*\*, and \*\*Deskflow\*\* — share a 15+ year old C++ codebase with recurring stability problems: connection drops, failed cursor swapping, broken clipboard sync, and occasional full-system hangs. InputSync is a clean-room rewrite in Rust with modern architectural decisions that eliminate entire classes of these bugs by design.



\*\*Target platforms:\*\* Windows 10/11 and Linux (X11 and Wayland).

\*\*Language:\*\* Rust 2021 edition.

\*\*License:\*\* TBD (likely GPL-3 or Apache-2.0).



\---



\## 2. Problems This Project Fixes



Each problem below is a known issue in existing tools that InputSync addresses through specific architectural choices, not through reactive patches.



\### 2.1 Connection drops and failed reconnects



\*\*Existing problem:\*\* Connections silently die after sleep/wake, network changes, or brief Wi-Fi drops. Manual restart often required. Reconnection logic is fragile or absent.



\*\*InputSync fix:\*\*

\- Transport built on \*\*QUIC\*\* (via `quinn`), which handles network roaming and migration natively.

\- \*\*Heartbeat protocol\*\* (Ping/Pong every 2 seconds) detects dead connections within 6 seconds.

\- \*\*Exponential-backoff auto-reconnect\*\* with state resynchronization on every reconnect.

\- Connection state is treated as inherently unreliable from day one — no assumption that "if connected, stays connected."



\### 2.2 Full-system hangs caused by input hooks



\*\*Existing problem:\*\* On Windows, low-level input hooks must return within \~300ms or the OS removes them. Existing tools sometimes do too much work inside the hook callback, causing the entire input subsystem to stall.



\*\*InputSync fix:\*\*

\- Hook callbacks do \*\*zero processing\*\* — they only push raw events into a lock-free queue (`crossbeam::channel`).

\- A separate tokio task drains the queue and processes events.

\- The hook can never block, regardless of network conditions or downstream load.



\### 2.3 Failed cursor swapping at screen edges



\*\*Existing problem:\*\* Cursor sometimes gets "stuck" at edges, fails to cross, or jumps back. Modifier keys (Shift, Ctrl) get stuck pressed on the wrong machine.



\*\*InputSync fix:\*\*

\- Explicit screen-edge state machine with deterministic transitions.

\- \*\*Modifier state synchronization\*\* on every screen enter/leave event — Shift held while crossing edges is released cleanly on the source machine and re-pressed on the target.

\- ESC-key escape hatch (configurable hotkey) forcibly returns control if the state machine glitches.



\### 2.4 Broken clipboard sync, especially with images and large content



\*\*Existing problem:\*\* Pasting large images freezes the source app. Clipboard formats are mishandled. Sync loops cause clipboards to flicker between machines.



\*\*InputSync fix:\*\*

\- \*\*Lazy clipboard transfer:\*\* only format metadata is broadcast on copy; actual content is fetched only when a paste is requested on the remote side.

\- \*\*Content hashing + originator tracking\*\* prevents sync loops.

\- Images are normalized to PNG on the wire. Text is normalized to UTF-8.

\- Configurable size cap (default 100MB) prevents runaway transfers.

\- Password-manager content (marked confidential) is never synced.



\### 2.5 Wayland incompatibility



\*\*Existing problem:\*\* InputLeap and friends are X11-only on Linux. Wayland — now default on Fedora, Ubuntu 24.04+, and most modern distros — intentionally blocks global input capture.



\*\*InputSync fix:\*\*

\- \*\*First-class Wayland support\*\* via libei (libinput emulation interface) and XDG RemoteDesktop / InputCapture portals.

\- Fallback chain: Portal → libei direct → uinput → X11 → clear error message.

\- X11 is supported but treated as the legacy path, not the primary path.



\### 2.6 GUI crashes taking down the input service



\*\*Existing problem:\*\* Existing tools mix GUI and input handling in one process. A GUI hiccup or crash interrupts input.



\*\*InputSync fix:\*\*

\- \*\*Daemon and GUI are separate binaries.\*\* The daemon runs as a Windows Service / systemd service and has no GUI dependencies.

\- GUI is optional; CLI control is fully supported.

\- IPC over named pipe (Windows) or Unix socket (Linux). GUI crashing is harmless to the daemon.



\### 2.7 Protocol fragility on version mismatch



\*\*Existing problem:\*\* Old clients connecting to new servers (or vice versa) sometimes corrupt state silently rather than failing cleanly.



\*\*InputSync fix:\*\*

\- Versioned binary protocol with explicit `version` field in every header.

\- \*\*Capability negotiation\*\* during handshake — clients only use features both sides support.

\- Version mismatch fails loudly with a clear error.



\### 2.8 Games and DirectInput applications miss inputs



\*\*Existing problem:\*\* Standard Windows hooks don't reach DirectInput-using games, making the tool useless for gaming setups.



\*\*InputSync fix:\*\*

\- Default: standard hooks (works for 95% of apps).

\- Optional: \*\*Interception driver\*\* support as a plugin for users who need game input.



\---



\## 3. Feature List



\### 3.1 Core features (v1.0)



| Feature | Description | Priority |

|---|---|---|

| Cross-machine cursor control | Mouse crosses edges between configured screens | Must-have |

| Keyboard forwarding | All keystrokes route to active machine | Must-have |

| Modifier key synchronization | Shift/Ctrl/Alt/Super state never desyncs | Must-have |

| Multi-monitor support | Each machine can have multiple physical monitors | Must-have |

| Configurable screen layout | Define which machine is on which side | Must-have |

| Encrypted connections | TLS 1.3 / QUIC with cert pinning | Must-have |

| Auto-reconnect | Handles sleep, network changes, brief drops | Must-have |

| Heartbeat health check | Detects dead connections fast | Must-have |

| Daemon + GUI separation | Service runs independent of UI | Must-have |

| Background service install | systemd unit + Windows Service | Must-have |

| Configuration file | TOML format, hot-reload on change | Must-have |

| Logging with rotation | `tracing` + file rotation | Must-have |

| Emergency hotkey | Configurable combo to force-release control | Must-have |

| CLI control tool | Headless config, status, start/stop | Must-have |



\### 3.2 Clipboard features (v1.0)



| Feature | Description |

|---|---|

| Text clipboard sync | UTF-8 plain text, both directions |

| Rich text clipboard sync | HTML and RTF preserved across machines |

| Image clipboard sync | Normalized to PNG, size-capped |

| Lazy transfer | Content fetched only when remote side pastes |

| Loop prevention | Content hashing + originator ID |

| Confidential content protection | Password-manager items not synced |

| Size cap | Configurable, default 100MB |



\### 3.3 File sharing features (v1.0)



| Feature | Description |

|---|---|

| File copy-paste | Ctrl+C on A, Ctrl+V on B, file transfers |

| Multi-file selection | Copy multiple files at once |

| Chunked transfer | 64KB chunks, blake3 hashed |

| Resume on disconnect | Transfers continue after network drops |

| Compression | zstd streaming, skipped for already-compressed files |

| Progress reporting | GUI shows transfer progress |

| Path sanitization | Server-side, prevents path traversal attacks |

| Concurrent transfers | Up to N simultaneous (configurable) |



\### 3.4 Platform features



| Feature | Windows | Linux X11 | Linux Wayland |

|---|---|---|---|

| Input capture | `SetWindowsHookEx` + Raw Input | `XGrabPointer` + XInput2 | libei + Portal |

| Input injection | `SendInput` | `XTestFakeKeyEvent` | libei + uinput |

| Clipboard | `AddClipboardFormatListener` | XFIXES + selections | `wl\_data\_device` |

| Multi-monitor | EDID + `EnumDisplayMonitors` | XRandR | `wl\_output` |

| Service install | Windows Service | systemd unit | systemd unit |

| Installer | MSI via `cargo-wix` | `.deb` / `.rpm` via `cargo-deb` | Same as X11 |



\### 3.5 Future features (post-v1.0)



\- Drag-and-drop file transfer (not just copy-paste)

\- Per-application input exclusions (auto-disable on game focus)

\- SSH-style host key trust UI

\- Multi-server topologies (3+ machines, not just 2)

\- macOS support

\- Mobile companion app (iOS/Android) as a tablet controller

\- Plugin system for custom input handlers

\- Audio routing between machines



\---



\## 4. Architecture and Communication



\### 4.1 High-level component diagram



```

┌─────────────────────────────────────────────────────────────────┐

│                      MACHINE A (Server)                        │

│                                                                 │

│  ┌────────────┐         ┌─────────────────────────────────┐   │

│  │   GUI      │◄───────►│         Daemon                  │   │

│  │  (egui)    │  IPC    │  ┌──────────────────────────┐   │   │

│  │            │ (Unix   │  │  Platform Layer          │   │   │

│  └────────────┘ socket/ │  │  (capture + inject)      │   │   │

│                  pipe)  │  └──────────────────────────┘   │   │

│                         │  ┌──────────────────────────┐   │   │

│  ┌────────────┐         │  │  Clipboard Manager       │   │   │

│  │   CLI      │◄───────►│  └──────────────────────────┘   │   │

│  └────────────┘         │  ┌──────────────────────────┐   │   │

│                         │  │  File Transfer Manager   │   │   │

│                         │  └──────────────────────────┘   │   │

│                         │  ┌──────────────────────────┐   │   │

│                         │  │  Network Layer (QUIC)    │   │   │

│                         │  └────────────┬─────────────┘   │   │

│                         └───────────────┼─────────────────┘   │

└─────────────────────────────────────────┼─────────────────────┘

&#x20;                                         │

&#x20;                             QUIC over UDP, TLS 1.3

&#x20;                                         │

┌─────────────────────────────────────────┼─────────────────────┐

│                      MACHINE B (Client) │                     │

│                         ┌───────────────┼─────────────────┐   │

│                         │  Network Layer (QUIC)           │   │

│                         │  ... (mirror of Machine A) ...  │   │

│                         └─────────────────────────────────┘   │

└─────────────────────────────────────────────────────────────────┘

```



\### 4.2 Three communication layers



InputSync has \*\*three distinct communication channels\*\*, each with its own protocol:



\#### Layer 1: Network protocol (machine ↔ machine)



\- \*\*Transport:\*\* QUIC over UDP, TLS 1.3 encryption built-in.

\- \*\*Why QUIC:\*\* native multi-stream support (input events, clipboard, file transfer on separate streams), connection migration across networks, faster reconnects than TCP+TLS.

\- \*\*Framing:\*\* Each message has a fixed header followed by a bincode-serialized payload.

\- \*\*Streams:\*\* Separate QUIC streams for input events (priority), clipboard, file transfer, and control messages. File transfers never block input events.



\*\*Header format (12 bytes):\*\*

```

┌────────────┬─────────┬──────────┬─────────────┐

│ magic (4)  │ ver (2) │ type (2) │ length (4)  │

│  "ISYN"    │  u16    │   u16    │     u32     │

└────────────┴─────────┴──────────┴─────────────┘

```



\*\*Message types\*\* (reserved IDs even if not implemented yet):

```

0x0001  Hello             0x0200  ClipboardFormats

0x0002  Welcome           0x0201  ClipboardRequest

0x0003  Goodbye           0x0202  ClipboardData

&#x20;                         

0x0100  MouseMove         0x0300  FileOfferStart

0x0101  MouseButton       0x0301  FileChunk

0x0102  MouseScroll       0x0302  FileAck

0x0103  KeyEvent          0x0303  FileTransferCancel

0x0110  ScreenEnter       

0x0111  ScreenLeave       0x0F00  Ping

0x0112  ModifierSync      0x0F01  Pong

&#x20;                         0x0F02  Error

```



\#### Layer 2: IPC protocol (daemon ↔ GUI/CLI)



\- \*\*Transport:\*\* Unix domain socket (Linux) or named pipe (Windows).

\- \*\*Path:\*\* `/run/inputsync/daemon.sock` (Linux), `\\\\.\\pipe\\inputsync` (Windows).

\- \*\*Authentication:\*\* filesystem permissions (Unix socket mode 0600 owned by the user; named pipe ACL on Windows).

\- \*\*Framing:\*\* same bincode-based framing as the network protocol, different message types.



\*\*IPC message types:\*\*

```

GetStatus              → Status { connected\_peers, uptime, version, ... }

GetConfig              → Config { ... }

UpdateConfig(Config)   → Result<(), Error>

Connect(PeerAddress)   → Result<(), Error>

Disconnect(PeerId)     → Result<(), Error>

SubscribeEvents        → stream of Event { connection, transfer\_progress, ... }

EmergencyStop          → Result<(), Error>

GetLogs(Filter)        → Vec<LogEntry>

```



\#### Layer 3: Internal channels (intra-daemon)



\- \*\*Transport:\*\* `tokio::sync::mpsc` and `crossbeam::channel` (for hook callbacks).

\- \*\*Pattern:\*\* event-driven actors. The input capture, clipboard watcher, file transfer manager, and network layer are independent tasks communicating through typed channels.

\- \*\*Backpressure:\*\* bounded channels with explicit drop-or-block policies. Input events have priority and use a separate channel from bulk data.



\### 4.3 Handshake sequence



```

Client (B)                                Server (A)

&#x20;  │                                         │

&#x20;  │──── QUIC connect (TLS 1.3) ───────────►│

&#x20;  │◄─── TLS handshake complete ────────────│

&#x20;  │                                         │

&#x20;  │──── Hello(client\_id, version, caps)───►│

&#x20;  │                                         │  verify cert pin

&#x20;  │                                         │  check version compat

&#x20;  │                                         │  negotiate capabilities

&#x20;  │                                         │

&#x20;  │◄─── Welcome(server\_id, accepted\_caps)──│

&#x20;  │                                         │

&#x20;  │◄═══ Ping/Pong every 2s ════════════════►│

&#x20;  │                                         │

&#x20;  │  (normal operation: input events,       │

&#x20;  │   clipboard sync, file transfers on     │

&#x20;  │   separate streams)                     │

&#x20;  │                                         │

```



\### 4.4 Data flow examples



\*\*Mouse move from Machine A to Machine B:\*\*

```

1\. OS event arrives at Machine A's hook callback.

2\. Hook pushes raw event to crossbeam::channel (returns in <1µs).

3\. Capture task drains channel, normalizes to InputEvent.

4\. Edge detector sees cursor at right edge of screen 1.

5\. Sends ScreenEnter to Machine B, switches routing to "remote".

6\. Subsequent MouseMove events serialized via bincode.

7\. Sent on input-priority QUIC stream to Machine B.

8\. Machine B deserializes, passes to inject task.

9\. inject task calls SendInput (Windows) or XTestFakeMotionEvent (Linux).

```



\*\*Clipboard sync (text):\*\*

```

1\. User copies text on Machine A.

2\. Windows fires WM\_CLIPBOARDUPDATE, Linux fires XFIXES SelectionNotify.

3\. Clipboard manager reads format list (NOT content yet).

4\. Hashes the format list + small preview.

5\. Broadcasts ClipboardFormats { formats, hash } to peers.

6\. Machine B receives, advertises matching formats to local apps.

7\. User presses Ctrl+V on Machine B.

8\. Local app requests clipboard data.

9\. Machine B sends ClipboardRequest { format: UTF8\_TEXT } to A.

10\. A reads actual content, sends ClipboardData.

11\. B delivers data to local app.

```



\*\*File transfer (copy-paste):\*\*

```

1\. User copies file on Machine A (Ctrl+C in file manager).

2\. Clipboard contains CF\_HDROP (Windows) or text/uri-list (Linux).

3\. Clipboard manager detects file URI, switches to file-transfer mode.

4\. Broadcasts ClipboardFormats with "files" capability.

5\. User pastes on Machine B.

6\. B sends FileTransferRequest.

7\. A sends FileOfferStart { manifest with sizes, hashes, names }.

8\. B sanitizes paths, allocates space, sends ack.

9\. A streams FileChunk messages on file-transfer QUIC stream.

10\. B writes chunks, periodically acks ranges.

11\. On disconnect, B remembers received chunks; on reconnect, A resumes.

12\. Once complete, B writes files to a configured drop location.

```



\### 4.5 Process and service model



\*\*Daemon process:\*\*

\- Runs as a system service (Windows Service / systemd unit).

\- Started automatically at boot.

\- Runs as the logged-in user on Linux (user systemd service), to access X11/Wayland session.

\- Has access to: filesystem, network, OS input APIs, clipboard.

\- Logs to: `%PROGRAMDATA%\\InputSync\\logs\\` (Windows), `\~/.local/state/inputsync/logs/` (Linux).



\*\*GUI process:\*\*

\- Launched manually or via OS startup.

\- Stateless — all state lives in the daemon.

\- Connects to daemon IPC socket on startup.

\- Can be killed and restarted without affecting input/clipboard/transfers.



\*\*CLI tool:\*\*

\- Same IPC client as GUI, but text-based.

\- For headless setups, scripting, debugging.



\### 4.6 Security model



\- \*\*Transport encryption:\*\* TLS 1.3 mandatory on all network connections.

\- \*\*Authentication:\*\* trust-on-first-use (TOFU). First connection shows certificate fingerprint, user confirms, fingerprint pinned in config file.

\- \*\*Authorization:\*\* only paired machines accepted. Unpaired connection attempts logged and rate-limited.

\- \*\*IPC security:\*\* daemon socket is user-owned with no group/world access. Only processes running as the same user can control the daemon.

\- \*\*Privilege:\*\* daemon does NOT run as root/Administrator. Uses uinput group on Linux and standard user APIs on Windows.

\- \*\*File transfer:\*\* receiver-side path sanitization. Sender cannot write outside the configured drop directory.

\- \*\*Clipboard:\*\* confidential-marked content (per OS hints) is never synced.



\---



\## 5. Repository Structure



```

inputsync/

├── Cargo.toml                       # Workspace definition

├── README.md                        # User-facing readme

├── LICENSE

├── docs/

│   ├── PROJECT\_OVERVIEW.md          # This document

│   ├── ARCHITECTURE.md              # Deep technical details

│   ├── PROTOCOL.md                  # Wire protocol spec

│   ├── BUILDING.md                  # How to build from source

│   └── CONTRIBUTING.md

├── crates/

│   ├── core/                        # Shared types, no OS deps

│   ├── protocol/                    # Wire format, bincode messages

│   ├── platform/                    # OS abstraction trait

│   ├── platform-windows/            # Windows backend

│   ├── platform-linux/              # Linux (X11 + Wayland) backend

│   ├── network/                     # QUIC + TLS transport

│   ├── clipboard/                   # Clipboard sync logic

│   ├── filetransfer/                # Chunked transfer + resume

│   ├── ipc/                         # Daemon ↔ GUI protocol

│   ├── daemon/                      # Service binary

│   ├── gui/                         # egui application

│   └── cli/                         # Command-line tool

├── packaging/

│   ├── windows/                     # MSI installer config

│   ├── linux/                       # systemd units, .deb/.rpm specs

│   └── icons/

├── tests/

│   └── integration/                 # End-to-end tests

└── .github/

&#x20;   └── workflows/                   # CI for Windows + Linux

```



\---



\## 6. Technology Stack Summary



| Concern | Choice |

|---|---|

| Language | Rust (2021 edition, MSRV 1.75+) |

| Async runtime | `tokio` (full features) |

| Network transport | `quinn` (QUIC) + `rustls` |

| Serialization | `bincode` v2 (wire), `serde` + `toml` (config) |

| Logging | `tracing` + `tracing-subscriber` + `tracing-appender` |

| Errors | `thiserror` (libs) + `anyhow` (binaries) |

| GUI | `egui` (v1), possible Tauri migration later |

| CLI | `clap` v4 derive |

| Hashing | `blake3` |

| Compression | `zstd` |

| Windows API | `windows` crate (official Microsoft) |

| Linux X11 | `x11rb` |

| Linux Wayland | `wayland-client` + `ashpd` (portals) + `reis` (libei) |

| Linux uinput | `input-linux` |

| Service mgmt | `windows-service` (Windows), systemd unit files (Linux) |

| Installer | `cargo-wix` (Windows MSI), `cargo-deb` / `cargo-rpm` (Linux) |

| CI | GitHub Actions, matrix: windows-latest + ubuntu-latest |



\---



\## 7. Development Roadmap Summary



| Phase | Focus | Duration |

|---|---|---|

| 0 | Foundation, workspace, CI | 1 week |

| 1 | Local input capture, one OS | 2 weeks |

| 2 | Local input injection, same OS | 1 week |

| 3 | Network protocol (plain TCP first) | 2 weeks |

| 4 | Cursor swapping, edge logic | 2 weeks |

| 5 | TLS, QUIC migration, auth | 2 weeks |

| 6 | Cross-platform — second OS | 3–4 weeks |

| 7 | Daemon + IPC + GUI split | 2–3 weeks |

| 8 | Clipboard sync | 2–3 weeks |

| 9 | File transfer | 2–3 weeks |

| 10 | Wayland support | 4–6 weeks |

| 11 | Polish, packaging, v1.0 | ongoing |



\*\*Realistic solo timeline:\*\* 6–9 months evenings/weekends to v1.0. 3–4 months full-time.



\---



\## 8. Success Criteria for v1.0



InputSync v1.0 ships when all of the following are true:



\- \[ ] Runs reliably on Windows 10/11 and a mainstream Linux distro (Ubuntu 24.04 LTS, Fedora current).

\- \[ ] Cursor and keyboard cross Windows↔Linux in both directions with no stuck modifiers.

\- \[ ] Connection survives sleep/wake cycles and Wi-Fi network changes without manual intervention.

\- \[ ] No reported full-system hang in 30 days of dogfooding.

\- \[ ] Clipboard text, HTML, and PNG images sync correctly in both directions.

\- \[ ] File copy-paste works for files up to 4GB with resume after network drops.

\- \[ ] Wayland (GNOME + KDE) input capture works via portal/libei.

\- \[ ] Encrypted (TLS 1.3) and authenticated (cert pinning) by default.

\- \[ ] MSI installer for Windows, .deb and .rpm for Linux.

\- \[ ] Daemon runs as system/user service and auto-starts.

\- \[ ] GUI crashes do not interrupt input or active transfers.

\- \[ ] Emergency hotkey reliably releases control in under 100ms.

\- \[ ] No known critical bugs in issue tracker.



\---



\## 9. Non-Goals (for v1.0)



To stay focused, these are explicitly \*\*out of scope\*\* for the first release:



\- macOS support (post-v1.0).

\- Mobile platforms (post-v1.0).

\- Audio routing.

\- Screen mirroring or video streaming.

\- Cloud-relay servers (LAN-only initially).

\- Plugin systems.

\- More than 4 machines in one cluster.

\- Drag-and-drop file transfer (copy-paste only in v1.0).

\- Configuration GUI feature parity with config file (CLI/file editing is the source of truth).



\---



\*This document is the source of truth for project scope and direction. It will be updated as decisions evolve.\*

