//! Windows input injection via `SendInput`.

use super::keymap::keycode_to_vk;
use crate::traits::Inject;
use async_trait::async_trait;
use inputsync_core::{Button, Error, InputEvent, KeyEvent, KeyState, MouseEvent, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub struct WindowsInject {
    _private: (),
}

impl WindowsInject {
    pub fn new() -> Self {
        Self { _private: () }
    }

    fn send(inputs: &[INPUT]) -> Result<()> {
        let written =
            unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if written as usize != inputs.len() {
            return Err(Error::Platform(format!(
                "SendInput wrote {} of {}",
                written,
                inputs.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Inject for WindowsInject {
    async fn inject(&self, event: InputEvent) -> Result<()> {
        match event {
            InputEvent::Mouse(MouseEvent::Move { x, y }) => move_absolute(x, y),
            InputEvent::Mouse(MouseEvent::MoveRelative { dx, dy }) => move_relative(dx, dy),
            InputEvent::Mouse(MouseEvent::Button { button, state }) => mouse_button(button, state),
            InputEvent::Mouse(MouseEvent::Scroll(delta)) => mouse_scroll(delta.horizontal, delta.vertical),
            InputEvent::Key(k) => key_event(k),
            // Sentinel events have no OS-level injection; handled at higher layer.
            InputEvent::ScreenEnter { .. }
            | InputEvent::ScreenLeave { .. }
            | InputEvent::ModifierSync(_) => Ok(()),
        }
    }

    async fn release_all_modifiers(&self) -> Result<()> {
        for vk in [
            VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN,
        ] {
            let _ = key_vk(vk, false);
        }
        Ok(())
    }
}

fn move_absolute(x: i32, y: i32) -> Result<()> {
    let virtual_w = unsafe { windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CXVIRTUALSCREEN) };
    let virtual_h = unsafe { windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CYVIRTUALSCREEN) };
    let nx = if virtual_w > 0 {
        ((x as i64 * 65535) / virtual_w as i64) as i32
    } else {
        0
    };
    let ny = if virtual_h > 0 {
        ((y as i64 * 65535) / virtual_h as i64) as i32
    } else {
        0
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    WindowsInject::send(&[input])
}

fn move_relative(dx: i32, dy: i32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    WindowsInject::send(&[input])
}

fn mouse_button(button: Button, state: KeyState) -> Result<()> {
    let down = matches!(state, KeyState::Pressed);
    let (flags, data) = match (button, down) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::Back, true) => (MOUSEEVENTF_XDOWN, 0x0001),
        (Button::Back, false) => (MOUSEEVENTF_XUP, 0x0001),
        (Button::Forward, true) => (MOUSEEVENTF_XDOWN, 0x0002),
        (Button::Forward, false) => (MOUSEEVENTF_XUP, 0x0002),
        (Button::Other(_), _) => return Ok(()),
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    WindowsInject::send(&[input])
}

fn mouse_scroll(horizontal: i16, vertical: i16) -> Result<()> {
    let mut inputs = Vec::new();
    if vertical != 0 {
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: vertical as i32 as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    if horizontal != 0 {
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: horizontal as i32 as u32,
                    dwFlags: MOUSEEVENTF_HWHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    if inputs.is_empty() {
        return Ok(());
    }
    WindowsInject::send(&inputs)
}

fn key_event(k: KeyEvent) -> Result<()> {
    let Some(vk) = keycode_to_vk(k.code) else {
        return Ok(());
    };
    key_vk(vk, matches!(k.state, KeyState::Pressed))
}

fn key_vk(vk: VIRTUAL_KEY, down: bool) -> Result<()> {
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
    let flags = if down {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan as u16,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    WindowsInject::send(&[input])
}
