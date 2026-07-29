//! Linux input injection via the kernel `/dev/uinput` interface.
//!
//! A virtual input device is created once at construction time; subsequent
//! `inject` calls write evdev events through it. Because the device is created
//! at the **kernel** level, the events look like real hardware to every
//! compositor — including Wayland, where application-level injection (XTEST)
//! is blocked.
//!
//! # Permissions
//! `/dev/uinput` is typically owned by `root:uinput` and mode `0660`, so the
//! process must either run as root or have its user in the `uinput` group.
//! The `.deb` installer's postinst adds the installing user to `input,uinput`.
//!
//! # Why absolute mouse moves become relative
//! uinput virtual pointers are relative devices by default (no abs axis is
//! declared). `MouseEvent::Move {x,y}` (absolute) is translated to a relative
//! warp from the last injected position; `MoveRelative` maps directly to
//! `REL_X`/`REL_Y`.

use crate::traits::Inject;
use async_trait::async_trait;
use input_linux::sys::{input_event, timeval};
use input_linux::{EventKind, InputId, Key, RelativeAxis, UInputHandle};
use inputsync_core::{Button, Error, InputEvent, KeyCode, KeyEvent, KeyState, MouseEvent, Result};
use parking_lot::Mutex;
use std::fs::OpenOptions;

/// evdev event types.
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const SYN_REPORT: u16 = 0;

/// Mouse button evdev codes (BTN_*). These live under EV_KEY, same as keys.
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_BACK: u16 = 0x116;
const BTN_FORWARD: u16 = 0x115;

/// BUS_VIRTUAL.
const BUS_VIRTUAL: u16 = 0x06;

/// A created uinput device. The handle keeps `/dev/uinput` open for the
/// lifetime of the injector.
pub struct LinuxInject {
    dev: Mutex<UInputHandle<std::fs::File>>,
    /// Last known absolute cursor position, used to convert absolute
    /// `Move` events into relative deltas.
    last_pos: Mutex<(i32, i32)>,
}

impl LinuxInject {
    pub fn new() -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    Error::Platform(
                        "permission denied opening /dev/uinput — add your user to the \
                         'uinput' and 'input' groups (the .deb installer does this), \
                         then log out and back in"
                            .into(),
                    )
                } else {
                    Error::Platform(format!("open /dev/uinput: {e}"))
                }
            })?;
        let dev = UInputHandle::new(file);

        // --- declare capabilities BEFORE creating the device ---
        // set_*bit all return io::Result<()>; we coerce to our Result.
        dev.set_evbit(EventKind::Key)
            .map_err(|e| io_err("set_evbit Key", e))?;
        dev.set_evbit(EventKind::Relative)
            .map_err(|e| io_err("set_evbit Relative", e))?;
        dev.set_evbit(EventKind::Synchronize)
            .map_err(|e| io_err("set_evbit Synchronize", e))?;

        // Full keyboard: iterate every Key variant. `Key` is repr(u16) over
        // evdev KEY_* codes; `is_key()` excludes mouse/gamepad buttons, so this
        // advertises the complete key range in one loop.
        for key in Key::iter() {
            if key.is_key() {
                // Best-effort: ignore codes the kernel rejects (some are
                // reserved/gaps). A failed set_keybit just means that key
                // won't be injectable, which is acceptable.
                let _ = dev.set_keybit(key);
            }
        }
        // Mouse buttons (BTN_* are buttons, excluded by is_key() above).
        for &btn in &[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_BACK, BTN_FORWARD] {
            if let Ok(key) = Key::from_code(btn) {
                dev.set_keybit(key)
                    .map_err(|e| io_err(&format!("set_keybit btn {btn:#x}"), e))?;
            }
        }
        // Relative axes for pointer movement + scroll (incl. hi-res wheels).
        for axis in [
            RelativeAxis::X,
            RelativeAxis::Y,
            RelativeAxis::Wheel,
            RelativeAxis::HorizontalWheel,
            RelativeAxis::WheelHiRes,
            RelativeAxis::HorizontalWheelHiRes,
        ] {
            dev.set_relbit(axis)
                .map_err(|e| io_err(&format!("set_relbit {axis:?}"), e))?;
        }

        // --- create the device via the high-level helper ---
        // `create` builds the uinput_setup struct internally (name as &[u8]),
        // tries the modern UI_DEV_SETUP path, and falls back to the legacy
        // uinput_user_dev write if the kernel needs it. abs is empty (we only
        // use relative axes). NUL-terminated name is required.
        let id = InputId {
            bustype: BUS_VIRTUAL,
            vendor: 0x0001,
            product: 0x0001,
            version: 0x0001,
        };
        dev.create(&id, b"InputSync virtual input\0", 0, &[])
            .map_err(|e| io_err("create", e))?;

        tracing::info!("uinput virtual device created");
        Ok(Self {
            dev: Mutex::new(dev),
            last_pos: Mutex::new((0, 0)),
        })
    }

    /// Write a batch of evdev events followed by a SYN_REPORT.
    fn write_batch(&self, mut events: Vec<input_event>) -> Result<()> {
        events.push(syn());
        let dev = self.dev.lock();
        let written = dev
            .write(&events)
            .map_err(|e| Error::Platform(format!("uinput write: {e}")))?;
        if written != events.len() {
            return Err(Error::Platform(format!(
                "uinput short write: {written}/{}",
                events.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Inject for LinuxInject {
    async fn inject(&self, event: InputEvent) -> Result<()> {
        match event {
            InputEvent::Mouse(MouseEvent::Move { x, y }) => {
                // Translate absolute -> relative from the last known position.
                let mut last = self.last_pos.lock();
                let dx = x - last.0;
                let dy = y - last.1;
                *last = (x, y);
                drop(last);
                if dx != 0 || dy != 0 {
                    self.write_batch(vec![
                        rel_event(RelativeAxis::X, dx),
                        rel_event(RelativeAxis::Y, dy),
                    ])?;
                }
                Ok(())
            }
            InputEvent::Mouse(MouseEvent::MoveRelative { dx, dy }) => {
                let mut last = self.last_pos.lock();
                last.0 = last.0.saturating_add(dx);
                last.1 = last.1.saturating_add(dy);
                drop(last);
                self.write_batch(vec![
                    rel_event(RelativeAxis::X, dx),
                    rel_event(RelativeAxis::Y, dy),
                ])
            }
            InputEvent::Mouse(MouseEvent::Button { button, state }) => {
                let Some(code) = button_to_evdev(button) else {
                    return Ok(()); // unmapped button: no-op (matches Windows backend)
                };
                self.write_batch(vec![key_event_raw(code, key_state_value(state))])
            }
            InputEvent::Mouse(MouseEvent::Scroll(delta)) => {
                let mut evs = Vec::with_capacity(4);
                if delta.vertical != 0 {
                    evs.push(rel_event(RelativeAxis::Wheel, delta.vertical as i32));
                    evs.push(rel_event(
                        RelativeAxis::WheelHiRes,
                        delta.vertical as i32 * 120,
                    ));
                }
                if delta.horizontal != 0 {
                    evs.push(rel_event(
                        RelativeAxis::HorizontalWheel,
                        delta.horizontal as i32,
                    ));
                    evs.push(rel_event(
                        RelativeAxis::HorizontalWheelHiRes,
                        delta.horizontal as i32 * 120,
                    ));
                }
                if evs.is_empty() {
                    return Ok(());
                }
                self.write_batch(evs)
            }
            InputEvent::Key(k) => inject_key(self, &k),
            InputEvent::ScreenEnter { .. }
            | InputEvent::ScreenLeave { .. }
            | InputEvent::ModifierSync(_) => Ok(()),
        }
    }

    async fn release_all_modifiers(&self) -> Result<()> {
        // Release left + right variants of each modifier, then one sync.
        let release_codes: [u16; 8] = [
            evdev_code(KeyCode::LeftCtrl),
            evdev_code(KeyCode::RightCtrl),
            evdev_code(KeyCode::LeftShift),
            evdev_code(KeyCode::RightShift),
            evdev_code(KeyCode::LeftAlt),
            evdev_code(KeyCode::RightAlt),
            evdev_code(KeyCode::LeftSuper),
            evdev_code(KeyCode::RightSuper),
        ];
        let mut events: Vec<input_event> =
            release_codes.iter().map(|&c| key_event_raw(c, 0)).collect();
        events.push(syn());
        let dev = self.dev.lock();
        dev.write(&events)
            .map_err(|e| Error::Platform(format!("uinput write modifiers: {e}")))?;
        Ok(())
    }
}

fn inject_key(inj: &LinuxInject, k: &KeyEvent) -> Result<()> {
    let Some(code) = hid_to_evdev(k.code) else {
        return Ok(()); // unmapped: silent skip (matches Windows backend)
    };
    inj.write_batch(vec![key_event_raw(code, key_state_value(k.state))])
}

// --------------------------- event builders ---------------------------

/// Zeroed `timeval` — uinput ignores event timestamps, so we don't need a
/// real clock read (which would require libc/syscall overhead per event).
fn zero_timeval() -> timeval {
    timeval {
        tv_sec: 0,
        tv_usec: 0,
    }
}

fn input_event_raw(type_: u16, code: u16, value: i32) -> input_event {
    input_event {
        time: zero_timeval(),
        type_,
        code,
        value,
    }
}

fn key_event_raw(code: u16, value: i32) -> input_event {
    input_event_raw(EV_KEY, code, value)
}

fn rel_event(axis: RelativeAxis, value: i32) -> input_event {
    input_event_raw(EV_REL, axis as u16, value)
}

fn syn() -> input_event {
    input_event_raw(EV_SYN, SYN_REPORT, 0)
}

fn key_state_value(state: KeyState) -> i32 {
    match state {
        KeyState::Pressed => 1,
        KeyState::Released => 0,
    }
}

// --------------------------- mappings ---------------------------

/// evdev keycode for a HID KeyCode via the standard offset. USB HID
/// usage-page-0x07 values are offset by 8 from evdev KEY_* values
/// (e.g. HID A=4 -> evdev KEY_A via the keyboard-range mapping below).
fn evdev_code(kc: KeyCode) -> u16 {
    kc as u16 + 8
}

/// Map a HID KeyCode to an evdev code. The HID keyboard usage range
/// (0x04..=0xE7) maps onto evdev KEY_* by raw_code + 8. Returns None for
/// `Unknown` and anything outside the keyboard range.
fn hid_to_evdev(kc: KeyCode) -> Option<u16> {
    let raw = kc as u16;
    if (4..=0xE7).contains(&raw) {
        Some(raw + 8)
    } else {
        None
    }
}

fn button_to_evdev(b: Button) -> Option<u16> {
    Some(match b {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
        Button::Back => BTN_BACK,
        Button::Forward => BTN_FORWARD,
        Button::Other(_) => return None,
    })
}

fn io_err(ctx: &str, e: std::io::Error) -> Error {
    Error::Platform(format!("uinput {ctx}: {e}"))
}
