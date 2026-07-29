# Windows packaging

## Service installation

InputSync registers as a Windows Service named `inputsync`. Install with:

```powershell
sc.exe create inputsync binPath= "\"C:\Program Files\InputSync\inputsync-daemon.exe\" run" start= auto
sc.exe start inputsync
```

Uninstall:

```powershell
sc.exe stop inputsync
sc.exe delete inputsync
```

## MSI installer

The MSI is produced with [`cargo-wix`](https://github.com/volks73/cargo-wix).
Run from the workspace root:

```powershell
cargo wix --no-build --package inputsync-daemon
```

This emits an `inputsync-X.Y.Z-x86_64.msi` in `target/wix/`.

## Permissions

The daemon runs as the logged-in user; no Administrator rights are needed
for normal operation. Low-level hooks (`SetWindowsHookEx`) require the
user's desktop session, which is why the service must be configured as a
user session service, not a session-0 service.
