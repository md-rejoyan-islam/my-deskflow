# Testing InputSync

This guide walks through every test you can run today, on each OS, with
honest notes on **what works** and **what's still a stub**. Use this as a
checklist before reporting bugs.

---

## 1. Current functional state

| Feature | Windows | Linux |
|---|---|---|
| Build & link the workspace | ✅ | ✅ |
| Run the daemon as a process | ✅ | ✅ |
| IPC (CLI ↔ daemon) | ✅ named pipe | ✅ Unix socket |
| QUIC server / client handshake | ✅ | ✅ |
| Cert generation + fingerprint pinning | ✅ | ✅ |
| Heartbeat (Ping/Pong) | ✅ | ✅ |
| **Capture local input** (mouse/keyboard) | ✅ `SetWindowsHookEx` | ❌ stub |
| **Inject remote input** | ✅ `SendInput` | ❌ stub |
| Edge detector / cursor swap | ✅ | n/a (no capture) |
| Clipboard text sync | ✅ via `arboard` | ✅ via `arboard` |
| Clipboard PNG sync | partial (read only) | partial (read only) |
| File transfer (send) | ✅ over QUIC bidi | ✅ |
| File transfer (receive + blake3 verify) | ✅ | ✅ |
| File transfer resume after disconnect | ❌ partial | ❌ partial |
| GUI status pane | ✅ | ✅ |
| Wayland support | n/a | ❌ not started |
| Service install (auto-start) | ❌ packaging only | ❌ packaging only |

**Bottom line:** end-to-end input sharing only works between two **Windows**
machines (or one Windows machine talking to itself on loopback). The Linux
side currently acts as **server-receiver** (gets input from Windows? — no,
because it can't inject) — really, Linux is for testing the network/CLI/
clipboard stack. Cross-OS input sharing is blocked on the Linux backend.

---

## 2. Build

```bash
cargo build --release --workspace
```

Outputs to `target/release/`:

- `inputsync-daemon` (or `.exe`)
- `inputsync-cli`
- `inputsync-gui`

For faster iteration during testing, use `cargo build` (debug profile) and
run from `target/debug/`. Replace `release` with `debug` in commands below.

---

## 3. One-machine loopback tests (start here)

These work the same on Windows and Linux and don't require a second
machine. Run each in its own terminal.

### 3.1 Daemon starts cleanly

```bash
inputsync-daemon run --role server --listen 127.0.0.1:24800
```

Expected log lines:

```
INFO ... local cert fingerprint fingerprint=<hex>
INFO ... ipc listening path=...
INFO ... QUIC server listening addr=127.0.0.1:24800
INFO ... daemon ready
INFO ... windows: capture started     # Windows only
```

✅ Pass = no errors, daemon stays up, Ctrl+C cleanly shuts it down.

### 3.2 CLI ↔ daemon IPC

In a second terminal:

```bash
inputsync-cli status
inputsync-cli config
inputsync-cli fingerprint
```

✅ Pass = JSON status, TOML config, hex fingerprint print without errors.

### 3.3 Loopback client connects to loopback server

Terminal 1 (server):

```bash
inputsync-daemon run --role server --listen 127.0.0.1:24800
```

Terminal 2 (grab the fingerprint):

```bash
inputsync-cli fingerprint
# → prints e.g. 04ab93d8d8d4...
```

Terminal 3 (client). Use `--socket` to give it a different IPC path so it
doesn't collide with the server's:

```bash
# Linux:
inputsync-daemon --config /tmp/client.toml run \
    --role client --connect 127.0.0.1:24800 \
    --pin <fingerprint>

# Windows: client and server can share the IPC pipe path; only the *first*
# daemon to bind it owns it. For local two-process tests, run the client
# with --no-ipc:
inputsync-daemon run --role client --connect 127.0.0.1:24800 \
    --pin <fingerprint> --no-ipc
```

Server log should show:

```
INFO  client connected remote=127.0.0.1:NNNNN
INFO  peer connected peer_id=<uuid> name=<host>
```

Client log:

```
INFO  connecting addr=127.0.0.1:24800
INFO  connected to server peer_id=<uuid> name=<host>
```

✅ Pass = both sides log the connection and stay running.

### 3.4 Bad fingerprint is rejected

Try the client without `--pin`, or with a wrong fingerprint:

```bash
inputsync-daemon run --role client --connect 127.0.0.1:24800 \
    --pin 0000000000000000000000000000000000000000000000000000000000000000 \
    --no-ipc
```

✅ Pass = client logs an error like `cert fingerprint ... not in pin list`
and exits the connection attempt.

### 3.5 Heartbeat traffic

With server + client running, set the log filter higher and watch for
Ping/Pong:

```bash
INPUTSYNC_LOG=trace inputsync-daemon run --role server --listen 127.0.0.1:24800
```

Look for `rx Pong` lines roughly every 2 seconds.

### 3.6 Emergency stop

```bash
inputsync-cli emergency
```

Should return `{"kind":"ok"}`. (No visible effect on loopback since no
input is currently being routed to a peer; this just exercises the IPC
path.)

### 3.7 Shutdown via IPC

```bash
inputsync-cli shutdown
```

The daemon should exit immediately. ✅ Pass = the daemon process is gone.

---

## 4. Two-Windows-machines test (the real test)

This is the one that exercises actual input sharing. You need two Windows
machines on the same LAN.

### 4.1 Setup

**Machine A** (the one whose keyboard/mouse you want to share):

```powershell
inputsync-daemon.exe run --role server --listen 0.0.0.0:24800
inputsync-cli.exe fingerprint
```

Note the fingerprint and Machine A's IP (`ipconfig`).

**Machine B** (the controlled one):

Open `%APPDATA%\InputSync\InputSync\config\inputsync.toml`. If it doesn't
exist:

```powershell
inputsync-daemon.exe init-config
```

Edit the layout section to declare that Machine A is on the LEFT:

```toml
[layout]

[[layout.screens]]
id = 0           # local (machine B)
name = "machine-b"
width = 1920
height = 1080

[[layout.screens]]
id = 1           # remote (machine A)
name = "machine-a"
width = 1920
height = 1080

[[layout.neighbours]]
from = 0
side = "Left"
to = 1
```

(Ignore for now — Machine B is the *receiver*, so its layout doesn't drive
edge detection. The layout above goes on **Machine A** so its edge detector
knows where Machine B sits.)

**Machine A** config edit — declare B is on the RIGHT:

```toml
[layout]

[[layout.screens]]
id = 0
name = "machine-a"
width = 1920
height = 1080

[[layout.screens]]
id = 1
name = "machine-b"
width = 1920
height = 1080

[[layout.neighbours]]
from = 0
side = "Right"
to = 1
```

Machine B (run as client):

```powershell
inputsync-daemon.exe run --role client --connect 192.168.1.10:24800 --pin <fingerprint-from-A>
```

### 4.2 Verify connection

On Machine A:

```powershell
inputsync-cli.exe status
```

Should show one entry under `connected_peers`.

### 4.3 Test cursor crossing (the headline feature)

1. On Machine A, move the mouse to the right edge of the screen.
2. The cursor should appear on Machine B and Machine A's local cursor
   should park.
3. Type some text on the keyboard — it should appear in whatever app has
   focus on Machine B.
4. Move the mouse back to the left edge of Machine B's screen — control
   should return to Machine A.

✅ Pass = cursor crosses cleanly, keys go to the right machine, no stuck
modifiers.

❌ Common failures:
- **Cursor crosses but keys don't follow** — capture is producing events
  but the routing isn't. Check `INPUTSYNC_LOG=debug` on Machine A for
  `cursor crossing to remote screen` log lines.
- **Cursor doesn't cross at all** — local screen geometry is wrong. Check
  the layout's `width` matches what Windows reports (`SM_CXVIRTUALSCREEN`).
- **Stuck Shift / Ctrl on Machine B** — modifier resync didn't fire. Press
  the emergency hotkey **Ctrl+Alt+Shift+Esc** on Machine A to force release.

### 4.4 Test clipboard sync

1. On Machine A, copy some text (`Ctrl+C`).
2. Wait ≤ 2 seconds.
3. On Machine B, open a text editor and paste (`Ctrl+V`).

✅ Pass = the text from A appears on B.

Reverse direction: copy on B, paste on A. Same expectation.

❌ Common failures:
- **Empty paste on the other machine** — check `inputsync-cli watch` for
  `ClipboardFormats` / `ClipboardData` traffic. Likely the poll loop didn't
  fire or `arboard` couldn't read the format.
- **Echo loop** (clipboard rapidly flips between machines) — the
  `OriginatorRegistry` is supposed to prevent this. File a bug with the
  log.

### 4.5 Test heartbeat survives a brief network drop

1. Start server + client connected.
2. On the client machine, briefly disable Wi-Fi or pull the cable for ~5s.
3. Restore the network.
4. Check `inputsync-cli status` on the server.

✅ Pass = the client reconnects automatically and re-appears in the peer
list (may take up to 30s due to backoff).

Note: this is **not** the same as the QUIC connection-migration story
promised for v1.0 — that requires more work on the reconnect path. Today
you'll see a fresh handshake.

### 4.6 Emergency hotkey

While Machine A's input is being routed to Machine B (cursor on B's
screen):

Press **Ctrl + Alt + Shift + Esc** on Machine A.

✅ Pass = control snaps back to Machine A immediately. Server log shows
`emergency hotkey: forcing routing to local`.

---

## 5. Linux tests (limited)

Capture and inject are stubs on Linux, so:

| Test | Works on Linux? |
|---|---|
| Build | ✅ |
| Daemon starts | ✅ |
| QUIC server/client handshake | ✅ |
| CLI ↔ daemon IPC (Unix socket) | ✅ |
| Clipboard sync (text, via arboard) | ✅ on X11; ⚠️ Wayland depends on portal |
| File transfer | ✅ |
| Cursor / keyboard sharing | ❌ — needs platform backend work |
| GUI | ✅ (status pane only) |

### 5.1 Smoke tests on Linux

Same as §3 above. The `--socket` flag uses a Unix socket at
`$XDG_RUNTIME_DIR/inputsync.sock` or `/tmp/inputsync.sock`.

### 5.2 Cross-platform handshake test

Run the server on Windows, client on Linux. The handshake should complete
and you should see the Linux client in `inputsync-cli status` on Windows.
Input won't flow (Linux can't inject), but the connection itself proves
the wire protocol works across OSes.

### 5.3 Clipboard text sync on Linux X11

1. Start two daemons (server on Linux machine, client on Windows machine).
2. Copy text on the Linux machine.
3. Paste on Windows.

This actually does work via `arboard`, since clipboard is OS-only and the
network protocol is the same.

---

## 6. Automated tests

```bash
cargo test --workspace
```

Currently 4 tests covering:
- Protocol frame roundtrip
- Bad-magic frame rejection
- Incomplete-frame detection
- Path sanitization (rejects `../`, absolute paths)

These are **fast** (<1s) and have no external dependencies. Run them
before every commit.

---

## 7. Manual test checklist (copy-paste for QA runs)

```
[ ] cargo build --release --workspace          (no errors)
[ ] cargo test --workspace                      (all pass)
[ ] inputsync-daemon fingerprint                (prints hex)
[ ] inputsync-daemon init-config                (writes TOML)
[ ] Server starts:        --role server         (logs "daemon ready")
[ ] CLI status reaches it                       (JSON returned)
[ ] CLI config reaches it                       (TOML returned)
[ ] Client connects (loopback)                  (peer count = 1)
[ ] Bad fingerprint rejected                    (client logs cert error)
[ ] Heartbeat ticks                             (Pong every 2s in trace log)
[ ] Two real machines connect (LAN)             (peer count = 1 both sides)
[ ] Cursor crosses A → B                        (visual confirm)
[ ] Keyboard follows                            (typing reaches B)
[ ] Cursor returns B → A                        (visual confirm)
[ ] No stuck modifiers after crossing           (Shift state correct on B)
[ ] Emergency hotkey releases                   (cursor snaps to A)
[ ] Clipboard text A → B                        (paste matches)
[ ] Clipboard text B → A                        (paste matches)
[ ] Brief network drop reconnects               (peer reappears in status)
[ ] CLI shutdown exits the daemon               (process gone)
```

---

## 8. Troubleshooting

### "address already in use"

A previous daemon is still listening. Find and kill it:

```powershell
# Windows
netstat -ano | findstr 24800
taskkill /F /PID <pid>
```

```bash
# Linux
ss -tunlp | grep 24800
kill <pid>
```

### "ipc peer disconnected" on client connect

Both daemons tried to bind the IPC socket/pipe. Run the second one with
`--no-ipc`, or use `--config /path/to/different.toml` so they pick
distinct OS user data dirs.

### Hooks not firing on Windows

`SetWindowsHookEx` requires the daemon to run in the **same desktop
session** as the user (not session 0). If you installed the daemon as a
LocalSystem service, the hooks won't see your input — run it from your
user account instead.

### `cert fingerprint X not in pin list`

The client's `--pin` doesn't match the server's actual cert hash. Re-run
`inputsync-cli fingerprint` on the server (or `inputsync-daemon
fingerprint`) and pass that exact hex.

### Linux: `arboard` clipboard fails

On Wayland you may need a desktop-portal-capable session. On headless
systems clipboard support is unavailable — that's expected.

### Port forwarding through a router

The default port is UDP 24800 (QUIC, not TCP). Forward UDP, not TCP. If
you must change it:

```bash
inputsync-daemon run --role server --listen 0.0.0.0:30000
inputsync-daemon run --role client --connect 1.2.3.4:30000 --pin ...
```

### Capturing logs to a file

```bash
INPUTSYNC_LOG=debug inputsync-daemon run --role server > server.log 2>&1
```

---

## 9. What's deliberately **not** testable yet

These features are documented in `summary.md` but have not landed:

- Wayland input capture / injection (libei + portals)
- Linux X11 input capture / injection (XTEST + XInput2)
- File transfer **resume after disconnect** (sender lookup of receiver
  offsets is not wired)
- Drag-and-drop file transfer (only programmatic API exists, no clipboard
  hookup)
- Multi-monitor edge detection (treats each machine as one logical screen)
- More than 2 machines in a cluster
- Cross-platform PNG clipboard write
- Service install (`install-service` subcommand is a TODO)
- Automatic mDNS / discovery (peers must know each other's IP)
- macOS support
- GUI: layout editor, pairing wizard, file-drop area
- Audio routing

If you find a bug in something on this list, that's expected — please file
the **feature** under "implement X" rather than as a bug.
