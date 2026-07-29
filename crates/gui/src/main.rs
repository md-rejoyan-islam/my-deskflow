//! InputSync GUI — a control panel over the daemon's IPC.
//!
//! Adapts to the daemon's role:
//! - **client** → renders a connection form (server address, fingerprint,
//!   Connect / Disconnect) plus the live connection state.
//! - **server** → renders the listening status and the connected-peers list.
//!
//! Status is polled once per second; Connect/Disconnect are fired as one-shot
//! IPC requests off the tokio runtime so the UI never blocks on the socket.

use anyhow::{Context, Result};
use eframe::egui;
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
        Self {
            socket_path,
            state: Arc::new(Mutex::new(SharedState::default())),
            runtime,
            poll_started: false,
            addr_input: String::new(),
            pin_input: String::new(),
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
            };
            let label = match action {
                Action::Connect { .. } => "Connect",
                Action::Disconnect => "Disconnect",
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

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("InputSync");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("socket: {}", self.socket_path.display()));
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
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
                ui.label("Start the daemon with `inputsync-daemon run --role server` (or client).");
                return;
            }
            let Some(status) = status else {
                ui.spinner();
                ui.label("Connecting to daemon…");
                return;
            };

            match status.role.as_str() {
                "client" => self.render_client_panel(ctx, ui, &status),
                _ => self.render_server_panel(ui, &status),
            }

            ui.separator();
            self.render_action_status(ui, last_action);

            ui.separator();
            ui.add_space(4.0);
            render_footer(ui, &status);
        });
    }
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

    fn render_server_panel(&mut self, ui: &mut egui::Ui, status: &StatusReply) {
        ui.heading("Server — listening for clients");

        egui::Grid::new("kv")
            .striped(true)
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Role");
                ui.label(&status.role);
                ui.end_row();
                ui.label("Uptime");
                ui.label(humantime(status.uptime_secs));
                ui.end_row();
                ui.label("Fingerprint");
                ui.monospace(&status.local_fingerprint);
                ui.end_row();
                ui.label("Capturing");
                ui.label(if status.capturing { "yes" } else { "no" });
                ui.end_row();
            });

        ui.separator();
        ui.heading(format!("Clients ({})", status.connected_peers.len()));
        if status.connected_peers.is_empty() {
            ui.label("no clients connected");
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
