//! InputSync GUI.
//!
//! Stateless front-end over the daemon's IPC: polls `GetStatus` and
//! renders peers, fingerprint, and capture state.

use anyhow::Context;
use eframe::egui;
use inputsync_ipc::{IpcClient, IpcRequest, IpcResponse, StatusReply};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 420.0])
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

struct InputSyncApp {
    socket_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
    runtime: tokio::runtime::Runtime,
    poll_started: bool,
}

#[derive(Default)]
struct SharedState {
    status: Option<StatusReply>,
    error: Option<String>,
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
}

async fn poll_once(socket: &PathBuf) -> anyhow::Result<StatusReply> {
    let mut client = IpcClient::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    match client.request(&IpcRequest::GetStatus).await? {
        IpcResponse::Status(s) => Ok(s),
        other => Err(anyhow::anyhow!("unexpected response: {:?}", other)),
    }
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
            let s = self.state.lock();
            if let Some(err) = &s.error {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("daemon unreachable: {err}"));
                ui.label("Start the daemon with `inputsync-daemon run --role server`.");
                return;
            }
            let Some(status) = &s.status else {
                ui.spinner();
                ui.label("Connecting to daemon…");
                return;
            };

            egui::Grid::new("kv").striped(true).num_columns(2).show(ui, |ui| {
                ui.label("Version");
                ui.label(&status.version);
                ui.end_row();
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
            ui.heading(format!("Peers ({})", status.connected_peers.len()));
            if status.connected_peers.is_empty() {
                ui.label("no peers connected");
            } else {
                for p in &status.connected_peers {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("•");
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
        });
    }
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
