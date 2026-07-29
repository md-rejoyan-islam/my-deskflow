//! Daemon lifecycle supervisor.
//!
//! The GUI owns the daemon process so the user never has to open a terminal:
//! when the GUI starts it [`DaemonSupervisor::ensure_running`]s the daemon, and
//! on a role change it writes the new config then [`restart`]s the child so it
//! picks up the new role.
//!
//! The daemon is a plain child process (no systemd/Service wrapper) — it lives
//! for as long as the GUI does, which is the simplest model that meets the
//! "double-click and go" requirement. Output is captured to pipes and dropped
//! (the daemon writes its own log files via `tracing-appender`).

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Instant;

/// The daemon binary name (no extension). On Windows the OS appends `.exe`.
const DAEMON_BIN_NAME: &str = "inputsync-daemon";

/// Owns the daemon child process and knows where its binary lives.
pub struct DaemonSupervisor {
    child: Option<Child>,
    binary: Option<PathBuf>,
    /// Throttles auto-relaunch attempts so we don't fork-bomb on a persistent
    /// failure (e.g. missing binary).
    last_launch_attempt: Option<Instant>,
}

impl DaemonSupervisor {
    pub fn new() -> Self {
        Self {
            child: None,
            binary: find_daemon_binary(),
            last_launch_attempt: None,
        }
    }

    /// Whether the daemon child is currently alive.
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(Some(_)) => false, // exited
                Ok(None) => true,     // still running
                Err(_) => false,
            },
            None => false,
        }
    }

    /// Start the daemon if it isn't already running. Idempotent.
    pub fn ensure_running(&mut self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        self.last_launch_attempt = Some(Instant::now());
        let bin = self.binary.as_ref().context(
            "daemon binary not found — install InputSync or run from a build output dir",
        )?;
        let mut cmd = Command::new(bin);
        cmd.arg("run");
        // Detach stdio so the child doesn't inherit (and block on) the GUI's
        // console. On Windows, CREATE_NO_WINDOW also suppresses a console flash.
        configure_detached(&mut cmd);
        let child = cmd
            .spawn()
            .with_context(|| format!("spawn daemon at {}", bin.display()))?;
        let pid = child.id();
        self.child = Some(child);
        // The GUI has no logging subscriber; a stderr line helps debugging
        // without pulling `tracing` into the GUI's dep tree.
        eprintln!("inputsync-gui: launched daemon (pid {pid})");
        Ok(())
    }

    /// Kill the current child (if any) and start a fresh one. Used after a
    /// role change: the daemon reads role once at startup, so the only way to
    /// apply a new role is to restart it.
    pub fn restart(&mut self) -> Result<()> {
        self.stop();
        self.ensure_running()
    }

    /// Kill and reap the child, if any.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Throttled auto-launch: only attempt if at least `cooldown_secs` have
    /// passed since the last attempt. Returns Ok(()) if it either launched
    /// successfully or is already running; surfaces errors but throttles the
    /// next attempt.
    pub fn ensure_running_throttled(&mut self, cooldown_secs: u64) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        if let Some(last) = self.last_launch_attempt {
            if last.elapsed().as_secs() < cooldown_secs {
                return Ok(()); // too soon since the last attempt
            }
        }
        self.ensure_running()
    }
}

impl Drop for DaemonSupervisor {
    fn drop(&mut self) {
        // Reap the child when the GUI closes so we don't orphan it.
        self.stop();
    }
}

/// Platform-specific child configuration: detached stdio + no console window.
/// stderr is redirected to a log file so daemon crashes can be diagnosed
/// (otherwise the GUI swallows all daemon output).
fn configure_detached(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW = 0x08000000
        cmd.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Put the child in its own process group so it doesn't receive the
        // GUI's Ctrl-C (it has its own shutdown handling via IPC Shutdown).
        cmd.process_group(0);
    }
    // stdout -> null. stderr -> a rotating log file next to the config so a
    // daemon crash leaves a trace the user (and we) can read.
    cmd.stdout(std::process::Stdio::null());
    if let Some(log) = daemon_log_path() {
        // Append so we keep history across restarts; ignore open errors
        // (falls back to inheriting stderr, which is harmless).
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            cmd.stderr(std::process::Stdio::from(file));
        }
    }
}

/// Where the GUI redirects the daemon's stderr. Lives next to the config so
/// it's easy to find: ~/.config/inputsync/daemon.stderr.log (or the OS
/// equivalent).
pub fn daemon_log_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("org", "InputSync", "InputSync")?;
    let dir = dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("daemon.stderr.log"))
}

/// Locate the daemon binary. Search order, most-installed first:
///   1. Well-known install paths (`/usr/bin`, `C:\Program Files\InputSync`).
///   2. Sibling of the running GUI executable (dev builds + same-dir installs).
///   3. `PATH` lookup via the OS `which`/`where`.
fn find_daemon_binary() -> Option<PathBuf> {
    // 1. Well-known install locations.
    for candidate in well_known_paths() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 2. Next to the GUI's own exe (e.g. target/release/, or a portable
    //    distribution where both binaries sit in one folder).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(daemon_filename());
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }

    // 3. PATH lookup. `which` (Unix) / `where` (Windows) print the resolved
    //    path on stdout, first line wins.
    if let Some(found) = lookup_on_path() {
        return Some(found);
    }

    None
}

/// The daemon filename with platform extension.
fn daemon_filename() -> String {
    #[cfg(windows)]
    {
        format!("{DAEMON_BIN_NAME}.exe")
    }
    #[cfg(not(windows))]
    {
        DAEMON_BIN_NAME.to_string()
    }
}

/// Hardcoded install paths per OS.
fn well_known_paths() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut v = Vec::new();
        if let Some(prog) = std::env::var_os("ProgramFiles") {
            v.push(
                PathBuf::from(prog)
                    .join("InputSync")
                    .join(format!("{DAEMON_BIN_NAME}.exe")),
            );
        }
        if let Some(prog) = std::env::var_os("ProgramFiles(x86)") {
            v.push(
                PathBuf::from(prog)
                    .join("InputSync")
                    .join(format!("{DAEMON_BIN_NAME}.exe")),
            );
        }
        v
    }
    #[cfg(unix)]
    {
        vec![
            PathBuf::from("/usr/bin").join(DAEMON_BIN_NAME),
            PathBuf::from("/usr/local/bin").join(DAEMON_BIN_NAME),
        ]
    }
}

/// Resolve the daemon via the OS path-search tool. Returns the first hit.
fn lookup_on_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let (tool, arg) = ("where", DAEMON_BIN_NAME);
    #[cfg(unix)]
    let (tool, arg) = ("which", DAEMON_BIN_NAME);

    let output = Command::new(tool).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}
