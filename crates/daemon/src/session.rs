//! Session orchestrator.
//!
//! In **server** mode: owns local capture, runs the edge detector, forwards
//! input events to the active peer.
//!
//! In **client** mode: receives input events from the server peer and feeds
//! them to the local injector.

use crate::clipboard_mgr::ClipboardManager;
use crate::edge::{EdgeDetector, ForwardTo, RouteDecision};
use crate::filetransfer_mgr::FileTransferManager;
use crate::heartbeat::spawn_heartbeat;
use inputsync_core::{
    Config, InputEvent, KeyCode, KeyEvent, KeyState, ModifierState, MouseEvent, PeerId, Point,
    ScreenId, ServerRole,
};
use inputsync_network::{ClientEvent, PeerHandle, ServerEvent};
use inputsync_platform::{traits::EventSink, CursorPos, Platform};
use inputsync_protocol::Message;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub struct Session {
    pub role: ServerRole,
    pub platform: Arc<dyn Platform>,
    pub config: Config,
    pub local_peer_id: PeerId,
    pub clipboard: Arc<ClipboardManager>,
    pub filetransfer: Arc<FileTransferManager>,
}

pub struct ServerSession {
    pub peer_tx: mpsc::Sender<ServerEvent>,
}

pub struct ClientSession {
    pub peer_tx: mpsc::Sender<ClientEvent>,
}

impl Session {
    pub fn new(
        role: ServerRole,
        platform: Arc<dyn Platform>,
        config: Config,
        local_peer_id: PeerId,
    ) -> Self {
        let drop_dir = config
            .file_transfer
            .drop_dir
            .clone()
            .unwrap_or_else(default_drop_dir);
        let clipboard = Arc::new(ClipboardManager::new(local_peer_id));
        let filetransfer = Arc::new(FileTransferManager::new(drop_dir));
        Self {
            role,
            platform,
            config,
            local_peer_id,
            clipboard,
            filetransfer,
        }
    }

    pub fn drop_dir(&self) -> PathBuf {
        self.filetransfer.drop_dir.clone()
    }

    pub fn spawn_server(self: Arc<Self>) -> ServerSession {
        let (peer_tx, peer_rx) = mpsc::channel::<ServerEvent>(32);
        let (capture_tx, capture_rx) = mpsc::channel::<InputEvent>(4096);

        let local = inputsync_platform::local_screen_info().unwrap_or(inputsync_core::ScreenInfo {
            id: ScreenId(0),
            name: "local".into(),
            width: 1920,
            height: 1080,
        });

        let edge = Arc::new(Mutex::new(EdgeDetector::new(
            local.id,
            local.width.max(1),
            local.height.max(1),
            self.config.layout.clone(),
        )));

        let peers: Arc<Mutex<Vec<PeerHandle>>> = Arc::new(Mutex::new(Vec::new()));

        // Spawn the clipboard poll loop so the server-side OS clipboard is
        // periodically broadcast to peers.
        if self.config.clipboard.enabled {
            self.clipboard.spawn_poll_loop(peers.clone());
        }

        // Start the platform capture in paused state.
        let capture = self.platform.capture();
        {
            let capture = capture.clone();
            let sink: Box<dyn EventSink> = Box::new(ChannelSink {
                tx: capture_tx.clone(),
            });
            tokio::spawn(async move {
                if let Err(e) = capture.start(sink).await {
                    warn!(error = %e, "platform capture start failed");
                }
            });
        }

        // Capture drain → edge detector → outbound to peer.
        {
            let edge = edge.clone();
            let peers = peers.clone();
            let capture = capture.clone();
            tokio::spawn(async move {
                drain_capture(capture_rx, edge, peers, capture).await;
            });
        }

        // Peer connect/disconnect / inbound.
        {
            let peers = peers.clone();
            let edge = edge.clone();
            let platform = self.platform.clone();
            let cfg = self.config.clone();
            let clipboard = self.clipboard.clone();
            let filetransfer = self.filetransfer.clone();
            tokio::spawn(async move {
                handle_peer_events_server(
                    peer_rx,
                    peers,
                    edge,
                    platform,
                    cfg,
                    clipboard,
                    filetransfer,
                )
                .await;
            });
        }

        ServerSession { peer_tx }
    }

    pub fn spawn_client(self: Arc<Self>) -> ClientSession {
        let (peer_tx, peer_rx) = mpsc::channel::<ClientEvent>(32);
        let platform = self.platform.clone();
        let cfg = self.config.clone();
        let clipboard = self.clipboard.clone();
        let filetransfer = self.filetransfer.clone();

        // The client also polls its OS clipboard and advertises changes to the
        // server — this gives bidirectional clipboard sync (copy on client →
        // paste on server, and vice versa). Without this, only the server's
        // clipboard was ever broadcast.
        let client_peers: Arc<Mutex<Vec<PeerHandle>>> = Arc::new(Mutex::new(Vec::new()));
        if cfg.clipboard.enabled {
            self.clipboard.spawn_poll_loop(client_peers.clone());
        }

        let client_peers2 = client_peers.clone();
        tokio::spawn(async move {
            handle_peer_events_client(
                peer_rx,
                platform,
                cfg,
                clipboard,
                filetransfer,
                client_peers2,
            )
            .await;
        });
        ClientSession { peer_tx }
    }
}

fn default_drop_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .map(|d| d.join("InputSync"))
        .unwrap_or_else(|| PathBuf::from("./inputsync-drop"))
}

struct ChannelSink {
    tx: mpsc::Sender<InputEvent>,
}

impl EventSink for ChannelSink {
    fn send(&self, event: InputEvent) {
        if let Err(e) = self.tx.try_send(event) {
            tracing::trace!(error = %e, "capture sink full");
        }
    }
}

async fn drain_capture(
    mut rx: mpsc::Receiver<InputEvent>,
    edge: Arc<Mutex<EdgeDetector>>,
    peers: Arc<Mutex<Vec<PeerHandle>>>,
    capture: Arc<dyn inputsync_platform::traits::Capture>,
) {
    // Tracks the last absolute mouse position so we can compute relative
    // deltas when forwarding moves to the remote peer (the local cursor is
    // pinned near the edge after a crossing, so absolute coords are useless
    // to the peer — it needs the movement delta).
    let mut last_mouse: Option<Point> = None;

    while let Some(event) = rx.recv().await {
        if is_emergency(&event) {
            warn!("emergency hotkey: forcing routing to local");
            last_mouse = None;
            let decision = edge.lock().force_local();
            apply_decision(&decision, &peers, &capture).await;
            continue;
        }

        let (decision, forward) = edge.lock().observe(&event);
        apply_decision(&decision, &peers, &capture).await;

        if forward == ForwardTo::Remote {
            let active_peer = peers.lock().first().cloned();
            if let Some(peer) = active_peer {
                // Convert absolute mouse-move to a relative delta before
                // forwarding. The local cursor is pinned near the screen edge
                // after a crossing, so the peer must see how far the user
                // *moved*, not where the cursor *is* (which barely changes).
                let msg = match &event {
                    InputEvent::Mouse(MouseEvent::Move { x, y }) => {
                        let cur = Point { x: *x, y: *y };
                        let (dx, dy) = match last_mouse {
                            Some(prev) => (cur.x - prev.x, cur.y - prev.y),
                            None => (0, 0),
                        };
                        last_mouse = Some(cur);
                        // Small deltas (noise from being pinned at the edge)
                        // are dropped to avoid jitter.
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        Message::from_input(InputEvent::Mouse(MouseEvent::MoveRelative { dx, dy }))
                    }
                    _ => Message::from_input(event.clone()),
                };
                let _ = peer.try_send(msg);
            }
        } else {
            // Track position while local so the first remote delta is correct.
            if let InputEvent::Mouse(MouseEvent::Move { x, y }) = &event {
                last_mouse = Some(Point { x: *x, y: *y });
            }
        }
    }
}

async fn apply_decision(
    decision: &RouteDecision,
    peers: &Arc<Mutex<Vec<PeerHandle>>>,
    capture: &Arc<dyn inputsync_platform::traits::Capture>,
) {
    match decision {
        RouteDecision::Stay => {}
        RouteDecision::EnterRemote {
            screen,
            entry,
            local_warp,
            modifiers,
        } => {
            info!(%screen, "cursor crossing to remote screen");
            let active_peer = peers.lock().first().cloned();
            if let Some(peer) = active_peer {
                let _ = peer
                    .send(Message::ScreenEnter {
                        x: entry.x,
                        y: entry.y,
                        modifiers: *modifiers,
                    })
                    .await;
            }
            if let Err(e) = capture.warp_cursor(CursorPos {
                x: local_warp.x,
                y: local_warp.y,
            }) {
                debug!(error = %e, "warp failed");
            }
            capture.set_capturing(true);
        }
        RouteDecision::LeaveRemote { screen } => {
            info!(%screen, "cursor returning to local");
            let active_peer = peers.lock().first().cloned();
            if let Some(peer) = active_peer {
                let _ = peer
                    .send(Message::ScreenLeave {
                        peer_screen: screen.0,
                    })
                    .await;
                let _ = peer
                    .send(Message::ModifierSync(ModifierState::empty()))
                    .await;
            }
            capture.set_capturing(false);
        }
    }
}

fn is_emergency(event: &InputEvent) -> bool {
    if let InputEvent::Key(k) = event {
        if matches!(k.state, KeyState::Pressed)
            && k.code == KeyCode::Escape
            && k.modifiers.contains(ModifierState::CTRL)
            && k.modifiers.contains(ModifierState::ALT)
            && k.modifiers.contains(ModifierState::SHIFT)
        {
            return true;
        }
    }
    false
}

async fn handle_peer_events_server(
    mut rx: mpsc::Receiver<ServerEvent>,
    peers: Arc<Mutex<Vec<PeerHandle>>>,
    edge: Arc<Mutex<EdgeDetector>>,
    platform: Arc<dyn Platform>,
    cfg: Config,
    clipboard: Arc<ClipboardManager>,
    filetransfer: Arc<FileTransferManager>,
) {
    while let Some(evt) = rx.recv().await {
        match evt {
            ServerEvent::Connected { handle, inbound } => {
                info!(peer_id = %handle.peer_id, name = %handle.peer_name, "peer connected");
                peers.lock().push(handle.clone());
                // Auto-route edge crossings to this peer when no explicit
                // layout is configured (the default). ScreenId(1) is the id
                // the server assigns to the first client (server.rs).
                edge.lock().set_auto_peer(Some(ScreenId(1)));
                spawn_heartbeat(handle.clone(), cfg.network.heartbeat_interval_ms);
                let platform = platform.clone();
                let clipboard = clipboard.clone();
                let filetransfer = filetransfer.clone();
                tokio::spawn(async move {
                    inbound_loop(handle, inbound, platform, false, clipboard, filetransfer).await;
                });
            }
            ServerEvent::Disconnected { peer_id } => {
                info!(%peer_id, "peer disconnected");
                peers.lock().retain(|p| p.peer_id != peer_id);
                // If no peers remain, clear the auto-peer so edge crossings
                // are no-ops again (don't route into a void).
                if peers.lock().is_empty() {
                    edge.lock().set_auto_peer(None);
                }
            }
        }
    }
}

async fn handle_peer_events_client(
    mut rx: mpsc::Receiver<ClientEvent>,
    platform: Arc<dyn Platform>,
    cfg: Config,
    clipboard: Arc<ClipboardManager>,
    filetransfer: Arc<FileTransferManager>,
    peers: Arc<Mutex<Vec<PeerHandle>>>,
) {
    while let Some(evt) = rx.recv().await {
        match evt {
            ClientEvent::Connected { handle, inbound } => {
                info!(peer_id = %handle.peer_id, name = %handle.peer_name, "connected to server");
                // Track the server peer so the clipboard poll loop can
                // broadcast clipboard changes to it (bidirectional sync).
                peers.lock().push(handle.clone());
                spawn_heartbeat(handle.clone(), cfg.network.heartbeat_interval_ms);
                let platform = platform.clone();
                let clipboard = clipboard.clone();
                let filetransfer = filetransfer.clone();
                tokio::spawn(async move {
                    inbound_loop(handle, inbound, platform, true, clipboard, filetransfer).await;
                });
            }
            ClientEvent::Disconnected { peer_id } => {
                info!(%peer_id, "server disconnected");
                peers.lock().retain(|p| p.peer_id != peer_id);
            }
        }
    }
}

async fn inbound_loop(
    peer: PeerHandle,
    mut inbound: mpsc::Receiver<Message>,
    platform: Arc<dyn Platform>,
    is_client: bool,
    clipboard: Arc<ClipboardManager>,
    filetransfer: Arc<FileTransferManager>,
) {
    let inject = platform.inject();
    while let Some(msg) = inbound.recv().await {
        match msg {
            Message::Ping {
                nonce,
                timestamp_ms,
            } => {
                let _ = peer
                    .send(Message::Pong {
                        nonce,
                        timestamp_ms,
                    })
                    .await;
            }
            Message::Pong { .. } => {}
            Message::MouseMove { x, y } if is_client => {
                let _ = inject
                    .inject(InputEvent::Mouse(MouseEvent::Move { x, y }))
                    .await;
            }
            Message::MouseButton(e) | Message::MouseScroll(e) if is_client => {
                let _ = inject.inject(InputEvent::Mouse(e)).await;
            }
            Message::KeyEvent(k) if is_client => {
                let _ = inject.inject(InputEvent::Key(k)).await;
            }
            Message::ScreenEnter { x, y, modifiers } if is_client => {
                let _ = inject
                    .inject(InputEvent::Mouse(MouseEvent::Move { x, y }))
                    .await;
                resync_modifiers(inject.as_ref(), modifiers).await;
            }
            Message::ScreenLeave { .. } if is_client => {
                let _ = inject.release_all_modifiers().await;
            }
            Message::ModifierSync(m) if is_client => {
                resync_modifiers(inject.as_ref(), m).await;
            }
            Message::ClipboardFormats { .. }
            | Message::ClipboardRequest { .. }
            | Message::ClipboardData(_) => {
                clipboard.handle_inbound(&peer, msg).await;
            }
            Message::FileOfferStart(o) => {
                if let Err(e) = filetransfer.handle_offer(o).await {
                    warn!(error = %e, "file offer failed");
                }
            }
            Message::FileChunk(c) => {
                if let Err(e) = filetransfer.handle_chunk(c).await {
                    warn!(error = %e, "file chunk failed");
                }
            }
            Message::FileTransferCancel {
                transfer_id,
                reason,
            } => {
                info!(%transfer_id, %reason, "file transfer cancelled by peer");
            }
            Message::FileAck { .. } => {}
            Message::Goodbye(g) => {
                info!(reason = %g.reason, "peer goodbye");
            }
            _ => {
                debug!(ty = ?msg.message_type(), is_client, "unhandled inbound");
            }
        }
    }
}

async fn resync_modifiers(inject: &dyn inputsync_platform::traits::Inject, target: ModifierState) {
    let _ = inject.release_all_modifiers().await;
    for (flag, code) in [
        (ModifierState::SHIFT, KeyCode::LeftShift),
        (ModifierState::CTRL, KeyCode::LeftCtrl),
        (ModifierState::ALT, KeyCode::LeftAlt),
        (ModifierState::SUPER, KeyCode::LeftSuper),
    ] {
        if target.contains(flag) {
            let _ = inject
                .inject(InputEvent::Key(KeyEvent {
                    code,
                    state: KeyState::Pressed,
                    modifiers: target,
                    character: None,
                }))
                .await;
        }
    }
}
