use serde::{Deserialize, Serialize};

/// Discriminator for the kind of input event being transported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Mouse(MouseEvent),
    Key(KeyEvent),
    /// Cursor entered the local screen from a particular edge of a peer.
    ScreenEnter { x: i32, y: i32, modifiers: ModifierState },
    /// Cursor left the local screen toward a peer.
    ScreenLeave { peer_screen: u32 },
    /// Periodic resync of held modifiers.
    ModifierSync(ModifierState),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseEvent {
    /// Absolute position, in the logical coordinate space of the source screen.
    Move { x: i32, y: i32 },
    /// Relative motion delta. Used inside games / locked-cursor scenarios.
    MoveRelative { dx: i32, dy: i32 },
    Button { button: Button, state: KeyState },
    Scroll(ScrollDelta),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollDelta {
    pub horizontal: i16,
    pub vertical: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub state: KeyState,
    pub modifiers: ModifierState,
    /// Best-effort UTF-32 character produced by the keystroke, if any.
    /// Used for fallback injection when the receiving OS lacks a direct
    /// keycode mapping (different layouts, dead keys, etc.).
    pub character: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyState {
    Pressed,
    Released,
}

/// Platform-neutral key identifier. Values match a subset of the USB HID
/// keyboard usage page (page 0x07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum KeyCode {
    Unknown = 0,
    // Letters
    A = 4,
    B = 5,
    C = 6,
    D = 7,
    E = 8,
    F = 9,
    G = 10,
    H = 11,
    I = 12,
    J = 13,
    K = 14,
    L = 15,
    M = 16,
    N = 17,
    O = 18,
    P = 19,
    Q = 20,
    R = 21,
    S = 22,
    T = 23,
    U = 24,
    V = 25,
    W = 26,
    X = 27,
    Y = 28,
    Z = 29,
    // Digits
    Num1 = 30,
    Num2 = 31,
    Num3 = 32,
    Num4 = 33,
    Num5 = 34,
    Num6 = 35,
    Num7 = 36,
    Num8 = 37,
    Num9 = 38,
    Num0 = 39,
    // Control
    Enter = 40,
    Escape = 41,
    Backspace = 42,
    Tab = 43,
    Space = 44,
    Minus = 45,
    Equals = 46,
    LeftBracket = 47,
    RightBracket = 48,
    Backslash = 49,
    Semicolon = 51,
    Apostrophe = 52,
    Grave = 53,
    Comma = 54,
    Period = 55,
    Slash = 56,
    CapsLock = 57,
    // Function row
    F1 = 58,
    F2 = 59,
    F3 = 60,
    F4 = 61,
    F5 = 62,
    F6 = 63,
    F7 = 64,
    F8 = 65,
    F9 = 66,
    F10 = 67,
    F11 = 68,
    F12 = 69,
    // Navigation
    PrintScreen = 70,
    ScrollLock = 71,
    Pause = 72,
    Insert = 73,
    Home = 74,
    PageUp = 75,
    Delete = 76,
    End = 77,
    PageDown = 78,
    ArrowRight = 79,
    ArrowLeft = 80,
    ArrowDown = 81,
    ArrowUp = 82,
    // Modifiers
    LeftCtrl = 224,
    LeftShift = 225,
    LeftAlt = 226,
    LeftSuper = 227,
    RightCtrl = 228,
    RightShift = 229,
    RightAlt = 230,
    RightSuper = 231,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
    pub struct ModifierState: u16 {
        const SHIFT      = 0b0000_0001;
        const CTRL       = 0b0000_0010;
        const ALT        = 0b0000_0100;
        const SUPER      = 0b0000_1000;
        const CAPS_LOCK  = 0b0001_0000;
        const NUM_LOCK   = 0b0010_0000;
        const ALT_GR     = 0b0100_0000;
    }
}
