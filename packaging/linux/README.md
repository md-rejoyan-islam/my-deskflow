# Linux packaging

## systemd user service

```bash
mkdir -p ~/.config/systemd/user
cp inputsync.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now inputsync.service
journalctl --user -u inputsync.service -f
```

The user service is preferred over a system service so the daemon inherits
the active graphical session (needed for X11 / Wayland clipboard and input
APIs).

## Required permissions

- Add your user to the `input` and `uinput` groups (Linux input injection).
- On Wayland, the desktop portal will prompt for screen-capture / input
  permission on first use.

## Building packages

`.deb` and `.rpm` packages can be built with
[`cargo-deb`](https://github.com/kornelski/cargo-deb) and
[`cargo-rpm`](https://github.com/iqlusioninc/cargo-rpm). Configuration goes
in the per-crate `Cargo.toml` (planned for v1.0).
