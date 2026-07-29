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
mod theme;

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
            .with_inner_size([820.0, 600.0])
            .with_min_inner_size([560.0, 420.0])
            .with_title("InputSync"),
        ..Default::default()
    };
    eframe::run_native(
        "InputSync",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(theme::visuals());
            theme::style(&cc.egui_ctx);
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

        egui::TopBottomPanel::top("top")
            .exact_height(56.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    // App name in accent color, bold.
                    ui.label(
                        egui::RichText::new("◆  InputSync")
                            .strong()
                            .color(theme::ACCENT)
                            .size(19.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::ghost_button(ui, "🔄 Change role") {
                            self.role_decided = false;
                        }
                    });
                });
                ui.add_space(4.0);
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
            ui.add_space(8.0);

            // Surface a supervisor error (e.g. binary not found) prominently.
            if let Some(err) = &self.supervisor_error {
                error_card(ui, "Daemon launch error", err, Some("Could not start the daemon automatically. Install InputSync, or if running from a build dir, make sure inputsync-daemon is next to inputsync-gui."));
                if theme::ghost_button(ui, "↻  Retry launch") {
                    self.supervisor_error = None;
                    let _ = self.supervisor.restart();
                }
                return;
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
                error_card(ui, "Daemon unreachable", err, Some("The daemon isn't responding to the GUI. You can try restarting it."));
                ui.add_space(8.0);
                if theme::accent_button(ui, "↻  Restart daemon") {
                    match self.supervisor.restart() {
                        Ok(()) => self.supervisor_error = None,
                        Err(e) => self.supervisor_error = Some(format!("{e:#}")),
                    }
                }
                if let Some(log) = daemon::daemon_log_path() {
                    ui.add_space(8.0);
                    theme::dim_label(ui, &format!("Daemon log: {}", log.display()));
                }
                return;
            }
            let Some(status) = status else {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.add_space(8.0);
                    theme::dim_label(ui, "Connecting to daemon…");
                });
                return;
            };

            ui.add_space(8.0);
            match status.role.as_str() {
                "client" => self.render_client_panel(ctx, ui, &status),
                _ => self.render_server_panel(ctx, ui, &status),
            }

            self.render_action_status(ui, last_action);

            ui.add_space(12.0);
            render_footer(ui, &status);
        });
    }
}

/// Render an error banner as a red-bordered card with an icon.
fn error_card(ui: &mut egui::Ui, title: &str, detail: &str, hint: Option<&str>) {
    egui::Frame::group(ui.style())
        .fill(theme::DANGER.linear_multiply(0.08))
        .stroke(egui::Stroke::new(1.0, theme::DANGER.linear_multiply(0.5)))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("⚠").color(theme::DANGER).size(20.0));
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).strong().color(theme::DANGER));
                    ui.label(egui::RichText::new(detail).color(theme::TEXT_PRIMARY));
                    if let Some(h) = hint {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(h).color(theme::TEXT_DIM).small());
                    }
                });
            });
        });
}

/// The first-run role picker. Takes over the central panel and asks the user
/// whether this computer should act as a server or a client. Returns the
/// chosen role, if any (the caller persists it + restarts the daemon).
fn render_role_picker(ui: &mut egui::Ui, supervisor_error: &Option<String>) -> Option<ServerRole> {
    let mut picked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.label(
            egui::RichText::new("Welcome to InputSync")
                .heading()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(4.0);
        theme::dim_label(ui, "Share one keyboard and mouse across two computers.");
        ui.add_space(32.0);
        theme::dim_label(ui, "How should this computer be used?");
        ui.add_space(20.0);

        // Two large selectable cards side by side.
        ui.horizontal_centered(|ui| {
            ui.add_space(20.0);
            // --- Server card ---
            let server_clicked = role_card(
                ui,
                "🖥",
                "Server",
                "This computer's keyboard and mouse\nwill control others. Run this on\nthe machine you sit at.",
                theme::ACCENT,
            );
            ui.add_space(16.0);
            // --- Client card ---
            let client_clicked = role_card(
                ui,
                "💻",
                "Client",
                "This computer will be controlled\nby a remote server. Run this on\nthe machine whose screen you want\nto reach.",
                theme::TEXT_DIM,
            );
            if server_clicked {
                picked = Some(ServerRole::Server);
            }
            if client_clicked {
                picked = Some(ServerRole::Client);
            }
        });

        if let Some(err) = supervisor_error {
            ui.add_space(20.0);
            ui.label(egui::RichText::new(err).color(theme::DANGER));
        }
    });
    picked
}

/// A single large role-selection card. Returns true if clicked.
fn role_card(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    desc: &str,
    accent: egui::Color32,
) -> bool {
    let desired = egui::Vec2::new(260.0, 160.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let hovered = response.hovered();
    let fill = if hovered {
        egui::Color32::from_rgb(28, 35, 48)
    } else {
        theme::BG_CARD
    };
    let stroke = if hovered {
        egui::Stroke::new(2.0, accent)
    } else {
        egui::Stroke::new(1.0, theme::BORDER)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(14.0), fill);
    ui.painter()
        .rect_stroke(rect, egui::Rounding::same(14.0), stroke);

    // Icon (large, centered-ish).
    let icon_pos = egui::pos2(rect.center().x, rect.top() + 38.0);
    ui.painter().text(
        icon_pos,
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(34.0),
        accent,
    );
    // Title.
    ui.painter().text(
        egui::pos2(rect.center().x, rect.top() + 80.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(18.0),
        theme::TEXT_PRIMARY,
    );
    // Description (dim, smaller).
    let galley = ui.fonts(|f| {
        f.layout(
            desc.to_string(),
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
            rect.width() - 24.0,
        )
    });
    ui.painter().galley(
        egui::pos2(rect.left() + 12.0, rect.top() + 104.0),
        galley,
        egui::Color32::TRANSPARENT,
    );
    response.clicked()
}

impl InputSyncApp {
    fn render_client_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        status: &StatusReply,
    ) {
        theme::heading(ui, "Client");
        theme::dim_label(ui, "Connect to a server to be controlled remotely.");
        ui.add_space(16.0);

        // --- Connection form card ---
        theme::card(ui, |ui| {
            egui::Grid::new("conn_form")
                .num_columns(2)
                .spacing([16.0, 12.0])
                .show(ui, |ui| {
                    theme::dim_label(ui, "Server address");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.addr_input)
                            .hint_text("192.168.1.50:24800")
                            .desired_width(280.0)
                            .clip_text(true),
                    );
                    ui.end_row();

                    theme::dim_label(ui, "Fingerprint");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pin_input)
                            .hint_text("paste the server's fingerprint hex")
                            .desired_width(280.0)
                            .code_editor()
                            .clip_text(true),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let connect_clicked = theme::accent_button(ui, "Connect");
                let disconnect_clicked = theme::danger_button(ui, "Disconnect");

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
        });

        ui.add_space(12.0);

        // --- Connection state ---
        theme::heading(
            ui,
            &format!("Connection ({})", status.connected_peers.len()),
        );
        ui.add_space(6.0);
        if status.connected_peers.is_empty() {
            theme::card(ui, |ui| {
                theme::dim_label(ui, "Not connected — fill the form and click Connect.");
            });
        } else {
            for p in &status.connected_peers {
                theme::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.painter().circle_filled(
                            ui.next_widget_position() + egui::vec2(5.0, 8.0),
                            4.0,
                            theme::SUCCESS,
                        );
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(&p.name)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.label(
                            egui::RichText::new(&p.peer_id)
                                .color(theme::TEXT_DIM)
                                .small()
                                .monospace(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(&format!("rtt {}ms", p.last_rtt_ms))
                                    .color(theme::TEXT_DIM)
                                    .small(),
                            );
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(&format!(
                            "remote {}  •  connected {}",
                            p.remote_addr,
                            humantime(p.connected_secs)
                        ))
                        .color(theme::TEXT_DIM)
                        .small(),
                    );
                });
                ui.add_space(6.0);
            }
        }
    }

    fn render_server_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        status: &StatusReply,
    ) {
        theme::heading(ui, "Server");
        theme::dim_label(ui, "Share this keyboard & mouse with other computers.");
        ui.add_space(16.0);

        // --- Status + Run/Stop card ---
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                if status.listening {
                    theme::pill(ui, "● Scanning for clients", theme::SUCCESS);
                } else {
                    theme::pill(ui, "● Idle", theme::WARNING);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !status.listening {
                        if theme::accent_button(ui, "▶  Run") {
                            self.dispatch_action(ctx.clone(), Action::StartServer);
                        }
                    } else if theme::danger_button(ui, "⏹  Stop") {
                        self.dispatch_action(ctx.clone(), Action::StopServer);
                    }
                });
            });
        });

        ui.add_space(12.0);

        // --- Connection info card (IP, port, fingerprint) ---
        theme::card(ui, |ui| {
            egui::Grid::new("server_info")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    theme::dim_label(ui, "Your IP address");
                    let ip_display = status
                        .listen_addr
                        .clone()
                        .unwrap_or_else(|| format!("{}  (click Run)", local_lan_ip()));
                    ui.label(
                        egui::RichText::new(&ip_display)
                            .color(theme::TEXT_PRIMARY)
                            .monospace(),
                    );
                    ui.end_row();

                    theme::dim_label(ui, "Port");
                    ui.label(
                        egui::RichText::new("24800")
                            .color(theme::TEXT_PRIMARY)
                            .monospace(),
                    );
                    ui.end_row();

                    theme::dim_label(ui, "Fingerprint");
                    ui.horizontal(|ui| {
                        let fp = if status.local_fingerprint.len() > 24 {
                            format!(
                                "{}…{}",
                                &status.local_fingerprint[..12],
                                &status.local_fingerprint[status.local_fingerprint.len() - 8..]
                            )
                        } else {
                            status.local_fingerprint.clone()
                        };
                        ui.label(
                            egui::RichText::new(&fp)
                                .color(theme::TEXT_PRIMARY)
                                .monospace(),
                        );
                        if theme::ghost_button(ui, "📋 Copy") {
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
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        // --- Connected clients ---
        theme::heading(ui, &format!("Clients ({})", status.connected_peers.len()));
        ui.add_space(6.0);
        if status.connected_peers.is_empty() {
            theme::card(ui, |ui| {
                if status.listening {
                    theme::dim_label(ui, "Waiting for a client to connect…");
                } else {
                    theme::dim_label(ui, "Server is not running. Click Run to start.");
                }
            });
        } else {
            for p in &status.connected_peers {
                theme::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.painter().circle_filled(
                            ui.next_widget_position() + egui::vec2(5.0, 8.0),
                            4.0,
                            theme::SUCCESS,
                        );
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(&p.name)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.label(
                            egui::RichText::new(&p.peer_id)
                                .color(theme::TEXT_DIM)
                                .small()
                                .monospace(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(&format!("rtt {}ms", p.last_rtt_ms))
                                    .color(theme::TEXT_DIM)
                                    .small(),
                            );
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(&format!(
                            "remote {}  •  connected {}",
                            p.remote_addr,
                            humantime(p.connected_secs)
                        ))
                        .color(theme::TEXT_DIM)
                        .small(),
                    );
                });
                ui.add_space(6.0);
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
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let color = if ok { theme::SUCCESS } else { theme::DANGER };
            ui.label(
                egui::RichText::new(if ok { "✓" } else { "✗" })
                    .color(color)
                    .strong(),
            );
            ui.label(egui::RichText::new(&message).color(color));
        });
    }
}

fn render_footer(ui: &mut egui::Ui, status: &StatusReply) {
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("InputSync v{}", status.version))
                .color(theme::TEXT_DIM)
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let fp = if status.local_fingerprint.len() > 24 {
                format!(
                    "{}…{}",
                    &status.local_fingerprint[..8],
                    &status.local_fingerprint[status.local_fingerprint.len() - 8..]
                )
            } else {
                status.local_fingerprint.clone()
            };
            ui.label(
                egui::RichText::new(&fp)
                    .color(theme::TEXT_DIM)
                    .small()
                    .monospace(),
            );
            ui.label(
                egui::RichText::new("fingerprint:")
                    .color(theme::TEXT_DIM)
                    .small(),
            );
        });
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
