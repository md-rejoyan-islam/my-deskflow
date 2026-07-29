//! Cursor edge detection and screen-routing state machine.
//!
//! Tracks where input is currently being routed: to the local OS, or to
//! one of the connected peer screens. On each mouse-move event we check
//! whether the cursor crossed a configured screen edge and emit a
//! [`RouteDecision`] for the orchestrator to act on.

use inputsync_core::{EdgeSide, InputEvent, ModifierState, MouseEvent, Point, ScreenId, ScreenLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Input stays on the local machine.
    Local,
    /// Input is forwarded to the peer driving this screen.
    Remote(ScreenId),
}

/// What the orchestrator should do as a side-effect of feeding this event
/// to the edge detector.
#[derive(Debug, Clone)]
pub enum RouteDecision {
    /// No routing change — forward (or drop) the event according to the
    /// current [`Route`].
    Stay,
    /// Routing flipped to a remote screen. The orchestrator should:
    /// 1. Park the local cursor at `local_warp`.
    /// 2. Send `ScreenEnter { x, y, modifiers }` to the remote peer.
    EnterRemote {
        screen: ScreenId,
        entry: Point,
        local_warp: Point,
        modifiers: ModifierState,
    },
    /// Routing flipped back to local. The orchestrator should:
    /// 1. Send `ScreenLeave` to the previously active peer.
    /// 2. Release any held modifiers on the peer's side.
    LeaveRemote { screen: ScreenId },
}

pub struct EdgeDetector {
    /// The local screen id (always 0 for now — multi-monitor on the local
    /// machine is rendered as one logical screen).
    local: ScreenId,
    local_width: i32,
    local_height: i32,
    layout: ScreenLayout,
    route: Route,
    last_point: Point,
    modifiers: ModifierState,
    /// Margin in px from the screen edge before a crossing is detected.
    /// 1 px is the strict default; some setups want a few px hysteresis.
    edge_threshold: i32,
}

impl EdgeDetector {
    pub fn new(local: ScreenId, local_width: i32, local_height: i32, layout: ScreenLayout) -> Self {
        Self {
            local,
            local_width,
            local_height,
            layout,
            route: Route::Local,
            last_point: Point { x: 0, y: 0 },
            modifiers: ModifierState::empty(),
            edge_threshold: 1,
        }
    }

    pub fn route(&self) -> Route {
        self.route
    }

    pub fn set_modifiers(&mut self, m: ModifierState) {
        self.modifiers = m;
    }

    /// Force routing back to local. Used by the emergency hotkey
    /// (summary §2.3 escape hatch).
    pub fn force_local(&mut self) -> RouteDecision {
        match self.route {
            Route::Local => RouteDecision::Stay,
            Route::Remote(screen) => {
                self.route = Route::Local;
                RouteDecision::LeaveRemote { screen }
            }
        }
    }

    /// Feed an input event. Returns the routing decision and a flag for
    /// whether the orchestrator should forward this event to the remote
    /// peer (true) or process it locally (false).
    pub fn observe(&mut self, event: &InputEvent) -> (RouteDecision, ForwardTo) {
        if let InputEvent::Key(k) = event {
            self.modifiers = k.modifiers;
        }
        if let InputEvent::ModifierSync(m) = event {
            self.modifiers = *m;
        }

        match event {
            InputEvent::Mouse(MouseEvent::Move { x, y }) => {
                self.last_point = Point { x: *x, y: *y };
                match self.route {
                    Route::Local => {
                        if let Some((side, edge_point)) =
                            self.cross_local_edge(self.last_point)
                        {
                            if let Some(remote) = self.layout.neighbour_of(self.local, side) {
                                // Entry point on the far screen: clamp to
                                // its proportional Y (or X) coordinate.
                                let entry = entry_point(side, edge_point);
                                let warp = warp_back_inside(
                                    side,
                                    self.local_width,
                                    self.local_height,
                                );
                                self.route = Route::Remote(remote);
                                return (
                                    RouteDecision::EnterRemote {
                                        screen: remote,
                                        entry,
                                        local_warp: warp,
                                        modifiers: self.modifiers,
                                    },
                                    ForwardTo::Remote,
                                );
                            }
                        }
                        (RouteDecision::Stay, ForwardTo::Local)
                    }
                    Route::Remote(_) => (RouteDecision::Stay, ForwardTo::Remote),
                }
            }
            _ => match self.route {
                Route::Local => (RouteDecision::Stay, ForwardTo::Local),
                Route::Remote(_) => (RouteDecision::Stay, ForwardTo::Remote),
            },
        }
    }

    fn cross_local_edge(&self, p: Point) -> Option<(EdgeSide, Point)> {
        if p.x <= self.edge_threshold {
            return Some((EdgeSide::Left, p));
        }
        if p.x >= self.local_width - self.edge_threshold {
            return Some((EdgeSide::Right, p));
        }
        if p.y <= self.edge_threshold {
            return Some((EdgeSide::Top, p));
        }
        if p.y >= self.local_height - self.edge_threshold {
            return Some((EdgeSide::Bottom, p));
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardTo {
    Local,
    Remote,
}

/// Point on the far screen where the cursor should appear after crossing.
fn entry_point(crossed: EdgeSide, local_pos: Point) -> Point {
    match crossed {
        EdgeSide::Left => Point { x: 32_000, y: local_pos.y },
        EdgeSide::Right => Point { x: 0, y: local_pos.y },
        EdgeSide::Top => Point { x: local_pos.x, y: 32_000 },
        EdgeSide::Bottom => Point { x: local_pos.x, y: 0 },
    }
}

/// Where the local cursor should be parked after we transfer control —
/// just inside the edge so it doesn't bounce back to remote on the next
/// move event.
fn warp_back_inside(crossed: EdgeSide, w: i32, h: i32) -> Point {
    match crossed {
        EdgeSide::Left => Point { x: 2, y: h / 2 },
        EdgeSide::Right => Point { x: w - 2, y: h / 2 },
        EdgeSide::Top => Point { x: w / 2, y: 2 },
        EdgeSide::Bottom => Point { x: w / 2, y: h - 2 },
    }
}
