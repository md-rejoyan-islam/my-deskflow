//! InputSync GUI — a control panel over the daemon's IPC.
//!
//! Adapts to the daemon's role:
//! - **client** → renders a connection form (server address, fingerprint,
//!   Connect / Disconnect) plus the live connection state.
//! - **server** → renders the listening status and the connected-peers list.
//!
//! The GUI also owns the daemon process (see [`daemon::DaemonSupervisor`]):
//! it auto-launches the daemon on startup so the user never needs a terminal,
//! and on a first run shows a role picker that persists the choice and
//! restarts the daemon to apply it.
//!
//! Status is polled once per second; Connect/Disconnect are fired as one-shot
//! IPC requests off the tokio runtime so the UI never blocks on the socket.

mod daemon;

use anyhow::{Context, Result};
use eframe::egui;
use inputsync_core::{Config, ServerRole};
use inputsync_ipc::{IpcClient, IpcRequest, IpcResponse, StatusReply};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("InputSync"),
        ..Default::default()
    };
    eframe::run_native(
        "InputSync",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(InputSyncApp::new()))
        }),
    )
}

/// A one-shot IPC action the GUI wants performed this frame.
#[derive(Clone)]
enum Action {
    Connect {
        addr: String,
        fingerprint: Option<String>,
    },
    Disconnect,
    /// Server role: start listening + capturing (the "Run" button).
    StartServer,
    /// Server role: stop listening + capturing (the "Stop" button).
    StopServer,
}

/// Result of an action, written from a background task into shared state.
struct ActionResult {
    ok: bool,
    message: String,
}

struct InputSyncApp {
    socket_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
    runtime: tokio::runtime::Runtime,
    poll_started: bool,
    /// Form inputs held on the UI thread.
    addr_input: String,
    pin_input: String,
    /// Owns the daemon child process + finds the binary.
    supervisor: daemon::DaemonSupervisor,
    /// True once we've decided a role exists (config file present or user
    /// picked one). Until then we show the first-run role picker overlay.
    role_decided: bool,
    /// Last supervisor error surfaced to the UI (e.g. binary not found).
    supervisor_error: Option<String>,
}

#[derive(Default)]
struct SharedState {
    status: Option<StatusReply>,
    error: Option<String>,
    /// Result + timestamp of the most recent Connect/Disconnect action.
    last_action: Option<(ActionResult, u64)>,
    /// Incremented whenever an action completes — used to nudge a repaint.
    action_seq: u64,
}

impl InputSyncApp {
    fn new() -> Self {
        let socket_path = inputsync_ipc::default_socket_path().unwrap_or_else(|_| PathBuf::new());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("tokio runtime");
        // First-run detection: if no config exists yet, show the role picker
        // before launching the daemon (so the daemon starts in the right role).
        let role_decided = Config::exists_at_default_path();
        Self {
            socket_path,
            state: Arc::new(Mutex::new(SharedState::default())),
            runtime,
            poll_started: false,
            addr_input: String::new(),
            pin_input: String::new(),
            supervisor: daemon::DaemonSupervisor::new(),
            role_decided,
            supervisor_error: None,
        }
    }

    /// Apply a first-run role choice: persist it to config and restart the
    /// daemon so it picks up the new role.
    fn apply_role_choice(&mut self, role: ServerRole) {
        let result = (|| -> Result<()> {
            let path = Config::default_path().context("config path")?;
            let mut cfg = Config::load_or_default(&path);
            cfg.role = role;
            cfg.save(&path).context("save config")?;
            // Role is read once at daemon startup, so restart to apply it.
            self.supervisor.restart()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.role_decided = true;
                self.supervisor_error = None;
            }
            Err(e) => {
                self.supervisor_error = Some(format!("{e:#}"));
            }
        }
    }

    fn start_poll_loop(&mut self, ctx: egui::Context) {
        let state = self.state.clone();
        let socket = self.socket_path.clone();
        self.runtime.spawn(async move {
            loop {
                match poll_once(&socket).await {
                    Ok(status) => {
                        let mut s = state.lock();
                        s.status = Some(status);
                        s.error = None;
                    }
                    Err(e) => {
                        let mut s = state.lock();
                        s.error = Some(e.to_string());
                    }
                }
                ctx.request_repaint();
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    /// Dispatch a one-shot IPC request off the UI thread, then record its
    /// outcome in shared state and request a repaint.
    fn dispatch_action(&self, ctx: egui::Context, action: Action) {
        let state = self.state.clone();
        let socket = self.socket_path.clone();
        self.runtime.spawn(async move {
            let req = match &action {
                Action::Connect { addr, fingerprint } => IpcRequest::Connect {
                    addr: addr.clone(),
                    fingerprint: fingerprint.clone(),
                },
                Action::Disconnect => IpcRequest::Disconnect {
                    peer: String::new(),
                },
                Action::StartServer => IpcRequest::StartServer,
                Action::StopServer => IpcRequest::StopServer,
            };
            let label = match action {
                Action::Connect { .. } => "Connect",
                Action::Disconnect => "Disconnect",
                Action::StartServer => "Start server",
                Action::StopServer => "Stop server",
            };
            let res = match send_request(&socket, &req).await {
                Ok(IpcResponse::Ok) => ActionResult {
                    ok: true,
                    message: format!("{label} accepted by daemon."),
                },
                Ok(IpcResponse::Error { message }) => ActionResult { ok: false, message },
                Ok(other) => ActionResult {
                    ok: false,
                    message: format!("unexpected response: {other:?}"),
                },
                Err(e) => ActionResult {
                    ok: false,
                    message: format!("{label} failed: {e}"),
                },
            };
            {
                let mut s = state.lock();
                let now = now_millis();
                s.last_action = Some((res, now));
                s.action_seq = s.action_seq.wrapping_add(1);
            }
            ctx.request_repaint();
        });
    }
}

async fn poll_once(socket: &PathBuf) -> Result<StatusReply> {
    let mut client = IpcClient::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    match client.request(&IpcRequest::GetStatus).await? {
        IpcResponse::Status(s) => Ok(s),
        other => Err(anyhow::anyhow!("unexpected response: {:?}", other)),
    }
}

async fn send_request(socket: &PathBuf, req: &IpcRequest) -> Result<IpcResponse> {
    let mut client = IpcClient::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    client.request(req).await
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl eframe::App for InputSyncApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.poll_started {
            self.start_poll_loop(ctx.clone());
            self.poll_started = true;
        }

        // Auto-launch the daemon once a role is decided (config present). We
        // do this BEFORE rendering so the poll loop has something to talk to.
        // Throttled to once per few seconds to avoid hammering on failure.
        if self.role_decided {
            if let Err(e) = self.supervisor.ensure_running_throttled(3) {
                self.supervisor_error = Some(format!("{e:#}"));
            } else if self.supervisor.is_running() {
                // Clear a stale error once the daemon is up again.
                if self.supervisor_error.is_some() {
                    self.supervisor_error = None;
                }
            }
            // Keep polling so the UI updates as soon as the daemon responds.
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("InputSync");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // "Change role" re-opens the first-run role picker. The
                    // chosen role is persisted + the daemon restarted to apply.
                    if ui.button("🔄 Change role").clicked() {
                        self.role_decided = false;
                    }
                    ui.separator();
                    ui.label(format!("socket: {}", self.socket_path.display()));
                });
            });
        });

        // First-run role picker overlay: takes over the whole panel until the
        // user chooses Server or Client.
        if !self.role_decided {
            let mut picked: Option<ServerRole> = None;
            egui::CentralPanel::default().show(ctx, |ui| {
                picked = render_role_picker(ui, &self.supervisor_error);
            });
            if let Some(role) = picked {
                self.apply_role_choice(role);
            }
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Surface a supervisor error (e.g. binary not found) prominently.
            if let Some(err) = &self.supervisor_error {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("daemon launch error: {err}"),
                );
                ui.label(
                    "Could not start the daemon automatically. Install InputSync, or if running \
                     from a build dir, make sure inputsync-daemon is next to inputsync-gui.",
                );
                if ui.button("Retry launch").clicked() {
                    self.supervisor_error = None;
                    let _ = self.supervisor.restart();
                }
                ui.separator();
            }

            // Snapshot under a short lock; render from the copies.
            let (error, status, last_action) = {
                let s = self.state.lock();
                (
                    s.error.clone(),
                    s.status.clone(),
                    s.last_action
                        .as_ref()
                        .map(|(r, t)| (r.ok, r.message.clone(), *t)),
                )
            };

            if let Some(err) = &error {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("daemon unreachable: {err}"),
                );
                ui.horizontal(|ui| {
                    ui.label("The daemon isn't responding. You can restart it here:");
                    if ui.button("Restart daemon").clicked() {
                        match self.supervisor.restart() {
                            Ok(()) => self.supervisor_error = None,
                            Err(e) => self.supervisor_error = Some(format!("{e:#}")),
                        }
                    }
                });
                if let Some(log) = daemon::daemon_log_path() {
                    ui.label(format!("Daemon log: {}", log.display()));
                }
                return;
            }
            let Some(status) = status else {
                ui.spinner();
                ui.label("Connecting to daemon…");
                return;
            };

            match status.role.as_str() {
                "client" => self.render_client_panel(ctx, ui, &status),
                _ => self.render_server_panel(ctx, ui, &status),
            }

            ui.separator();
            self.render_action_status(ui, last_action);

            ui.separator();
            ui.add_space(4.0);
            render_footer(ui, &status);
        });
    }
}

/// The first-run role picker. Takes over the central panel and asks the user
/// whether this computer should act as a server or a client. Returns the
/// chosen role, if any (the caller persists it + restarts the daemon).
fn render_role_picker(ui: &mut egui::Ui, supervisor_error: &Option<String>) -> Option<ServerRole> {
    let mut picked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.heading("Welcome to InputSync");
        ui.label("Share one keyboard and mouse across two computers.");
        ui.add_space(20.0);
        ui.label("How should this computer be used?");
        ui.add_space(16.0);

        ui.horizontal(|ui| {
            ui.add_space(80.0);
            if ui
                .add(egui::Button::new("🖥  Server").min_size(egui::vec2(140.0, 60.0)))
                .on_hover_text(
                    "This computer's keyboard and mouse will control others. \
                     Run this on the machine you sit at.",
                )
                .clicked()
            {
                picked = Some(ServerRole::Server);
            }
            if ui
                .add(egui::Button::new("💻  Client").min_size(egui::vec2(140.0, 60.0)))
                .on_hover_text(
                    "This computer will be controlled by a remote server. \
                     Run this on the machine whose screen you want to reach.",
                )
                .clicked()
            {
                picked = Some(ServerRole::Client);
            }
        });

        if let Some(err) = supervisor_error {
            ui.add_space(16.0);
            ui.colored_label(egui::Color32::LIGHT_RED, format!("{err}"));
        }
    });
    picked
}

impl InputSyncApp {
    fn render_client_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        status: &StatusReply,
    ) {
        ui.heading("Client — connect to a server");
        ui.add_space(6.0);

        egui::Grid::new("conn_form")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Server address");
                ui.add(
                    egui::TextEdit::singleline(&mut self.addr_input)
                        .hint_text("192.168.1.50:24800")
                        .desired_width(260.0)
                        .clip_text(true),
                );
                ui.end_row();

                ui.label("Server fingerprint");
                ui.add(
                    egui::TextEdit::singleline(&mut self.pin_input)
                        .hint_text("paste the server's fingerprint hex")
                        .desired_width(360.0)
                        .code_editor()
                        .clip_text(true),
                );
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let connect_clicked = ui
                .add(egui::Button::new("Connect"))
                .on_hover_text("Dial the server address above (replaces any current connection).")
                .clicked();
            let disconnect_clicked = ui
                .add(egui::Button::new("Disconnect"))
                .on_hover_text("Hang up the current connection.")
                .clicked();

            if connect_clicked {
                let addr = self.addr_input.trim().to_string();
                if addr.is_empty() {
                    let mut s = self.state.lock();
                    s.last_action = Some((
                        ActionResult {
                            ok: false,
                            message: "Enter a server address first.".into(),
                        },
                        now_millis(),
                    ));
                } else {
                    let fp = {
                        let t = self.pin_input.trim().to_string();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    };
                    self.dispatch_action(
                        ctx.clone(),
                        Action::Connect {
                            addr,
                            fingerprint: fp,
                        },
                    );
                }
            }
            if disconnect_clicked {
                self.dispatch_action(ctx.clone(), Action::Disconnect);
            }
        });

        ui.add_space(10.0);
        ui.separator();

        // Live connection state.
        ui.heading(format!("Connection ({})", status.connected_peers.len()));
        if status.connected_peers.is_empty() {
            ui.label("Not connected — fill the form and click Connect.");
        } else {
            for p in &status.connected_peers {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("●");
                        ui.label(&p.name);
                        ui.monospace(&p.peer_id);
                    });
                    ui.label(format!("remote: {}", p.remote_addr));
                    ui.label(format!(
                        "connected: {}  •  rtt: {} ms",
                        humantime(p.connected_secs),
                        p.last_rtt_ms
                    ));
                });
            }
        }
    }

    fn render_server_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        status: &StatusReply,
    ) {
        ui.heading("Server — share this keyboard & mouse");
        ui.add_space(6.0);

        // Status line + Run/Stop control.
        ui.horizontal(|ui| {
            if status.listening {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 220, 120),
                    "● Scanning for clients…",
                );
            } else {
                ui.colored_label(egui::Color32::from_rgb(220, 180, 80), "● Idle");
            }
        });

        ui.add_space(4.0);

        // The IP address + port the clients should connect to.
        egui::Grid::new("server_info")
            .striped(true)
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Your IP address");
                let ip_display = status
                    .listen_addr
                    .clone()
                    .unwrap_or_else(|| format!("{} (click Run to start)", local_lan_ip()));
                ui.monospace(&ip_display);
                ui.end_row();

                ui.label("Port");
                ui.monospace("24800");
                ui.end_row();
            });

        ui.add_space(4.0);

        // Fingerprint — copyable.
        ui.horizontal(|ui| {
            ui.label("Fingerprint:");
            ui.monospace(&status.local_fingerprint);
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(status.local_fingerprint.clone());
                let mut s = self.state.lock();
                s.last_action = Some((
                    ActionResult {
                        ok: true,
                        message: "Fingerprint copied to clipboard.".into(),
                    },
                    now_millis(),
                ));
            }
        });

        ui.add_space(8.0);

        // Run / Stop buttons.
        ui.horizontal(|ui| {
            if !status.listening {
                if ui
                    .add(
                        egui::Button::new("▶  Run")
                            .min_size(egui::vec2(120.0, 36.0))
                            .fill(egui::Color32::from_rgb(60, 140, 80)),
                    )
                    .on_hover_text("Start listening for client connections and capturing input.")
                    .clicked()
                {
                    self.dispatch_action(ctx.clone(), Action::StartServer);
                }
            } else {
                if ui
                    .add(
                        egui::Button::new("⏹  Stop")
                            .min_size(egui::vec2(120.0, 36.0))
                            .fill(egui::Color32::from_rgb(170, 60, 60)),
                    )
                    .on_hover_text("Stop listening and release input capture.")
                    .clicked()
                {
                    self.dispatch_action(ctx.clone(), Action::StopServer);
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();

        // Connected clients.
        ui.heading(format!(
            "Connected clients ({})",
            status.connected_peers.len()
        ));
        if status.connected_peers.is_empty() {
            if status.listening {
                ui.label("Waiting for a client to connect…");
            } else {
                ui.label("Server is not running. Click Run to start.");
            }
        } else {
            for p in &status.connected_peers {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("●");
                        ui.label(&p.name);
                        ui.monospace(&p.peer_id);
                    });
                    ui.label(format!("remote: {}", p.remote_addr));
                    ui.label(format!(
                        "connected: {}  •  rtt: {} ms",
                        humantime(p.connected_secs),
                        p.last_rtt_ms
                    ));
                });
            }
        }
    }

    fn render_action_status(&self, ui: &mut egui::Ui, last_action: Option<(bool, String, u64)>) {
        let Some((ok, message, ts)) = last_action else {
            return;
        };
        // Only show recent results (last ~8s) so stale messages fade.
        let age = now_millis().saturating_sub(ts);
        if age > 8_000 {
            return;
        }
        ui.horizontal(|ui| {
            let color = if ok {
                egui::Color32::from_rgb(120, 220, 120)
            } else {
                egui::Color32::LIGHT_RED
            };
            ui.colored_label(
                color,
                format!("{}  {}", if ok { "✓" } else { "✗" }, message),
            );
        });
    }
}

fn render_footer(ui: &mut egui::Ui, status: &StatusReply) {
    ui.horizontal(|ui| {
        ui.label(format!("v{}", status.version));
        ui.separator();
        ui.label("Your fingerprint:");
        ui.monospace(&status.local_fingerprint);
    });
}

fn humantime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Best-effort local LAN IPv4 address for display. Tries UDP socket trick
/// (bind to a public IP, read local addr — doesn't actually send packets),
/// falls back to the hostname resolution.
fn local_lan_ip() -> String {
    // The "connect a UDP socket to a faraway address" trick returns the local
    // interface that would be used to reach it, without sending any packets.
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                let ip = addr.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    return ip.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}
