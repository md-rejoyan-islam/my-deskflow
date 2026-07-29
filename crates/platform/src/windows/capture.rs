//! Windows low-level keyboard / mouse hooks.
//!
//! Architecture (per summary §2.2): the hook callbacks themselves do zero
//! work — they only push the raw event onto a [`crossbeam_channel`], which
//! is lock-free and bounded. A worker thread drains the channel,
//! normalizes the event, and forwards it to the async [`EventSink`].
//!
//! This guarantees the hook returns in microseconds and Windows never
//! evicts it for timeout.

use super::keymap::vk_to_keycode;
use crate::traits::{Capture, EventSink};
use crate::CursorPos;
use async_trait::async_trait;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use inputsync_core::{
    Button, Error, InputEvent, KeyEvent, KeyState, ModifierState, MouseEvent, Result, ScrollDelta,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HMODULE, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Compact raw event pushed by the hook into the lock-free channel.
#[derive(Debug, Clone, Copy)]
enum RawEvent {
    KeyDown { vk: u32, scan: u32 },
    KeyUp { vk: u32, scan: u32 },
    MouseMove { x: i32, y: i32 },
    MouseButton { button: Button, down: bool },
    MouseScroll { horizontal: bool, delta: i16 },
}

impl RawEvent {
    #[allow(dead_code)]
    fn scan_code(&self) -> Option<u32> {
        match self {
            RawEvent::KeyDown { scan, .. } | RawEvent::KeyUp { scan, .. } => Some(*scan),
            _ => None,
        }
    }
}

/// Channel sender used by the hook callbacks. `static` so the C ABI hook
/// procs can reach it.
static HOOK_TX: Mutex<Option<Sender<RawEvent>>> = Mutex::new(None);
static HOOK_CAPTURING: AtomicBool = AtomicBool::new(false);
static KEYBOARD_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);

pub struct WindowsCapture {
    state: Arc<CaptureState>,
}

struct CaptureState {
    running: AtomicBool,
    capturing: AtomicBool,
    hook_thread: Mutex<Option<JoinHandle<()>>>,
    drain_thread: Mutex<Option<JoinHandle<()>>>,
}

impl WindowsCapture {
    pub fn new() -> Self {
        Self {
            state: Arc::new(CaptureState {
                running: AtomicBool::new(false),
                capturing: AtomicBool::new(false),
                hook_thread: Mutex::new(None),
                drain_thread: Mutex::new(None),
            }),
        }
    }
}

#[async_trait]
impl Capture for WindowsCapture {
    async fn start(&self, sink: Box<dyn EventSink>) -> Result<()> {
        if self.state.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let (tx, rx) = bounded::<RawEvent>(8192);
        *HOOK_TX.lock() = Some(tx);

        // Spawn the hook thread. Hooks must be installed on a thread with
        // a Windows message loop, so we run one here.
        let state = self.state.clone();
        let hook_handle = std::thread::Builder::new()
            .name("inputsync-hooks".into())
            .spawn(move || hook_thread_main(state))
            .map_err(|e| Error::Platform(format!("spawn hook thread: {e}")))?;
        *self.state.hook_thread.lock() = Some(hook_handle);

        // Spawn the drain thread. Translates RawEvent → InputEvent and
        // forwards to the sink. Runs at normal priority; hook does not
        // block on it.
        let state2 = self.state.clone();
        let drain_handle = std::thread::Builder::new()
            .name("inputsync-drain".into())
            .spawn(move || drain_thread_main(state2, rx, sink))
            .map_err(|e| Error::Platform(format!("spawn drain thread: {e}")))?;
        *self.state.drain_thread.lock() = Some(drain_handle);

        tracing::info!("windows: capture started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.state.running.store(false, Ordering::SeqCst);
        self.state.capturing.store(false, Ordering::SeqCst);
        HOOK_CAPTURING.store(false, Ordering::SeqCst);

        // Tear down hooks by posting a message to the hook thread.
        let kbd = KEYBOARD_HOOK.swap(0, Ordering::SeqCst);
        let mouse = MOUSE_HOOK.swap(0, Ordering::SeqCst);
        unsafe {
            if kbd != 0 {
                let _ = UnhookWindowsHookEx(windows::Win32::UI::WindowsAndMessaging::HHOOK(
                    kbd as *mut _,
                ));
            }
            if mouse != 0 {
                let _ = UnhookWindowsHookEx(windows::Win32::UI::WindowsAndMessaging::HHOOK(
                    mouse as *mut _,
                ));
            }
        }

        // Drop the sender to wake the drain thread.
        *HOOK_TX.lock() = None;

        if let Some(h) = self.state.drain_thread.lock().take() {
            let _ = h.join();
        }
        if let Some(h) = self.state.hook_thread.lock().take() {
            let _ = h.join();
        }
        Ok(())
    }

    fn set_capturing(&self, capturing: bool) {
        self.state.capturing.store(capturing, Ordering::SeqCst);
        HOOK_CAPTURING.store(capturing, Ordering::SeqCst);
    }

    fn cursor_position(&self) -> Result<CursorPos> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).map_err(|e| Error::Platform(format!("GetCursorPos: {e}")))?;
            Ok(CursorPos { x: p.x, y: p.y })
        }
    }

    fn warp_cursor(&self, pos: CursorPos) -> Result<()> {
        unsafe {
            SetCursorPos(pos.x, pos.y)
                .map_err(|e| Error::Platform(format!("SetCursorPos: {e}")))?;
        }
        Ok(())
    }
}

fn hook_thread_main(state: Arc<CaptureState>) {
    unsafe {
        let module = HMODULE::default();
        let kbd = SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), module, 0);
        match (kbd, mouse) {
            (Ok(k), Ok(m)) => {
                KEYBOARD_HOOK.store(k.0 as isize, Ordering::SeqCst);
                MOUSE_HOOK.store(m.0 as isize, Ordering::SeqCst);
            }
            _ => {
                tracing::error!("failed to install windows hooks");
                state.running.store(false, Ordering::SeqCst);
                return;
            }
        }

        // Message pump: hooks fire on this thread when a low-level event
        // arrives, but we still need to spin a GetMessage loop or Windows
        // won't dispatch them.
        let mut msg = MSG::default();
        while state.running.load(Ordering::SeqCst) && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn drain_thread_main(state: Arc<CaptureState>, rx: Receiver<RawEvent>, sink: Box<dyn EventSink>) {
    let mods: parking_lot::Mutex<ModifierState> = parking_lot::Mutex::new(ModifierState::empty());

    while state.running.load(Ordering::SeqCst) {
        let raw = match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(r) => r,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        };

        // Update modifier state from key events even when not capturing —
        // we want a fresh snapshot to ship to the peer when capture starts.
        if let RawEvent::KeyDown { vk, .. } | RawEvent::KeyUp { vk, .. } = raw {
            let is_down = matches!(raw, RawEvent::KeyDown { .. });
            update_modifiers(&mut mods.lock(), vk, is_down);
        }

        if !state.capturing.load(Ordering::SeqCst) {
            continue;
        }

        let event: Option<InputEvent> = match raw {
            RawEvent::KeyDown { vk, .. } => {
                let modifiers = *mods.lock();
                Some(InputEvent::Key(KeyEvent {
                    code: vk_to_keycode(VIRTUAL_KEY(vk as u16)),
                    state: KeyState::Pressed,
                    modifiers,
                    character: None,
                }))
            }
            RawEvent::KeyUp { vk, .. } => {
                let modifiers = *mods.lock();
                Some(InputEvent::Key(KeyEvent {
                    code: vk_to_keycode(VIRTUAL_KEY(vk as u16)),
                    state: KeyState::Released,
                    modifiers,
                    character: None,
                }))
            }
            RawEvent::MouseMove { x, y } => Some(InputEvent::Mouse(MouseEvent::Move { x, y })),
            RawEvent::MouseButton { button, down } => Some(InputEvent::Mouse(MouseEvent::Button {
                button,
                state: if down {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
            })),
            RawEvent::MouseScroll { horizontal, delta } => {
                Some(InputEvent::Mouse(MouseEvent::Scroll(ScrollDelta {
                    horizontal: if horizontal { delta } else { 0 },
                    vertical: if horizontal { 0 } else { delta },
                })))
            }
        };

        if let Some(ev) = event {
            sink.send(ev);
        }
    }
}

fn update_modifiers(state: &mut ModifierState, vk: u32, down: bool) {
    let flag = match VIRTUAL_KEY(vk as u16) {
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT => ModifierState::SHIFT,
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL => ModifierState::CTRL,
        VK_LMENU | VK_RMENU | VK_MENU => ModifierState::ALT,
        VK_LWIN | VK_RWIN => ModifierState::SUPER,
        VK_CAPITAL => ModifierState::CAPS_LOCK,
        VK_NUMLOCK => ModifierState::NUM_LOCK,
        _ => return,
    };
    if down {
        state.insert(flag);
    } else {
        state.remove(flag);
    }
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let event = match wparam.0 as u32 {
            x if x == WM_KEYDOWN || x == WM_SYSKEYDOWN => Some(RawEvent::KeyDown {
                vk: kb.vkCode,
                scan: kb.scanCode,
            }),
            x if x == WM_KEYUP || x == WM_SYSKEYUP => Some(RawEvent::KeyUp {
                vk: kb.vkCode,
                scan: kb.scanCode,
            }),
            _ => None,
        };
        if let Some(ev) = event {
            try_push(ev);
        }
        // When capturing, swallow the event so it doesn't reach the
        // foreground app on this machine.
        if HOOK_CAPTURING.load(Ordering::SeqCst) {
            return LRESULT(1);
        }
    }
    let hook = HHOOK(KEYBOARD_HOOK.load(Ordering::SeqCst) as *mut _);
    CallNextHookEx(hook, code, wparam, lparam)
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let event = match wparam.0 as u32 {
            WM_MOUSEMOVE => Some(RawEvent::MouseMove {
                x: ms.pt.x,
                y: ms.pt.y,
            }),
            WM_LBUTTONDOWN => Some(RawEvent::MouseButton {
                button: Button::Left,
                down: true,
            }),
            WM_LBUTTONUP => Some(RawEvent::MouseButton {
                button: Button::Left,
                down: false,
            }),
            WM_RBUTTONDOWN => Some(RawEvent::MouseButton {
                button: Button::Right,
                down: true,
            }),
            WM_RBUTTONUP => Some(RawEvent::MouseButton {
                button: Button::Right,
                down: false,
            }),
            WM_MBUTTONDOWN => Some(RawEvent::MouseButton {
                button: Button::Middle,
                down: true,
            }),
            WM_MBUTTONUP => Some(RawEvent::MouseButton {
                button: Button::Middle,
                down: false,
            }),
            WM_MOUSEWHEEL => Some(RawEvent::MouseScroll {
                horizontal: false,
                delta: ((ms.mouseData >> 16) & 0xFFFF) as i16,
            }),
            WM_MOUSEHWHEEL => Some(RawEvent::MouseScroll {
                horizontal: true,
                delta: ((ms.mouseData >> 16) & 0xFFFF) as i16,
            }),
            _ => None,
        };
        if let Some(ev) = event {
            try_push(ev);
        }
        if HOOK_CAPTURING.load(Ordering::SeqCst) {
            return LRESULT(1);
        }
    }
    let hook = HHOOK(MOUSE_HOOK.load(Ordering::SeqCst) as *mut _);
    CallNextHookEx(hook, code, wparam, lparam)
}

fn try_push(ev: RawEvent) {
    if let Some(tx) = HOOK_TX.lock().as_ref() {
        match tx.try_send(ev) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!("input event queue full, dropping event");
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}
