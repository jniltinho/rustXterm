//! Pure input encoders: keys, mouse reports and paste (xterm conventions).

use egui::{Key, Modifiers};

/// xterm modifier parameter (`1 + shift + 2*alt + 4*ctrl`), `None` without modifiers.
fn modifier_param(m: Modifiers) -> Option<u8> {
    let p = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    (p > 1).then_some(p)
}

/// Bytes for a key press, `None` when the key produces nothing (or arrives as `Event::Text`).
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let esc_prefix = |mut v: Vec<u8>| {
        if m.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    // CSI/SS3 keys: arrows, Home/End, F1-F4.
    let cursor_key = |c: u8| -> Vec<u8> {
        match modifier_param(m) {
            Some(p) => format!("\x1b[1;{p}{}", c as char).into_bytes(),
            None if app_cursor => vec![0x1b, b'O', c],
            None => vec![0x1b, b'[', c],
        }
    };
    let ss3_key = |c: u8| -> Vec<u8> {
        match modifier_param(m) {
            Some(p) => format!("\x1b[1;{p}{}", c as char).into_bytes(),
            None => vec![0x1b, b'O', c],
        }
    };
    let tilde_key = |n: u8| -> Vec<u8> {
        match modifier_param(m) {
            Some(p) => format!("\x1b[{n};{p}~").into_bytes(),
            None => format!("\x1b[{n}~").into_bytes(),
        }
    };
    let bytes = match key {
        Key::ArrowUp => cursor_key(b'A'),
        Key::ArrowDown => cursor_key(b'B'),
        Key::ArrowRight => cursor_key(b'C'),
        Key::ArrowLeft => cursor_key(b'D'),
        Key::Home => cursor_key(b'H'),
        Key::End => cursor_key(b'F'),
        Key::F1 => ss3_key(b'P'),
        Key::F2 => ss3_key(b'Q'),
        Key::F3 => ss3_key(b'R'),
        Key::F4 => ss3_key(b'S'),
        Key::F5 => tilde_key(15),
        Key::F6 => tilde_key(17),
        Key::F7 => tilde_key(18),
        Key::F8 => tilde_key(19),
        Key::F9 => tilde_key(20),
        Key::F10 => tilde_key(21),
        Key::F11 => tilde_key(23),
        Key::F12 => tilde_key(24),
        Key::Insert => tilde_key(2),
        Key::Delete => tilde_key(3),
        Key::PageUp => tilde_key(5),
        Key::PageDown => tilde_key(6),
        Key::Enter => esc_prefix(vec![b'\r']),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc_prefix(vec![b'\t']),
        Key::Backspace if m.ctrl => esc_prefix(vec![0x08]),
        Key::Backspace => esc_prefix(vec![0x7f]),
        Key::Escape => vec![0x1b],
        // Ctrl+_ (Ctrl+Shift+-): the same byte as Ctrl+/; Ctrl+- itself zooms the font.
        Key::Minus if m.ctrl && m.shift => esc_prefix(vec![0x1f]),
        // Ctrl+Shift+letter is left to application shortcuts (copy/paste).
        _ if m.ctrl && !m.shift => esc_prefix(vec![ctrl_byte(key)?]),
        _ => return None,
    };
    Some(bytes)
}

/// Control byte for Ctrl+`key`.
fn ctrl_byte(key: Key) -> Option<u8> {
    let name = key.name().as_bytes();
    if let [c @ b'A'..=b'Z'] = name {
        return Some(c - b'A' + 1);
    }
    Some(match key {
        Key::Space | Key::Num2 => 0x00,
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Slash => 0x1f,
        _ => return None,
    })
}

/// Mouse button codes used in reports.
pub const BUTTON_LEFT: u8 = 0;
pub const BUTTON_MIDDLE: u8 = 1;
pub const BUTTON_RIGHT: u8 = 2;
/// "No button" (motion without buttons, X10 release).
pub const BUTTON_NONE: u8 = 3;
pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;
/// Added to the button code for motion events.
pub const MOTION: u8 = 32;

/// Modifier bits added to the button code.
pub fn mouse_modifiers(m: Modifiers) -> u8 {
    4 * u8::from(m.shift) + 8 * u8::from(m.alt) + 16 * u8::from(m.ctrl)
}

/// Encode a mouse event at 0-based cell (`col`, `row`). `code` already includes modifier
/// and motion bits. SGR reports releases with `m`; X10 reports them as button 3 and
/// cannot encode positions beyond column/row 223.
pub fn mouse_report(sgr: bool, code: u8, col: usize, row: usize, pressed: bool) -> Option<Vec<u8>> {
    if sgr {
        let end = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    let code = if pressed { code } else { (code & !0b11) | BUTTON_NONE };
    let enc = |v: usize| u8::try_from(v + 1 + 32).ok();
    Some(vec![0x1b, b'[', b'M', 32 + code, enc(col)?, enc(row)?])
}

/// Bytes for pasting `text`: newlines become CR and, with bracketed paste, the text is
/// wrapped in `ESC [200~ ... ESC [201~` (embedded end markers are removed).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text.into_bytes();
    }
    let mut out = b"\x1b[200~".to_vec();
    out.extend(text.replace("\x1b[201~", "").bytes());
    out.extend(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL, false), Some(vec![0x03]));
        assert_eq!(key_bytes(Key::A, Modifiers::CTRL, false), Some(vec![0x01]));
        assert_eq!(key_bytes(Key::Z, Modifiers::CTRL, false), Some(vec![0x1a]));
        assert_eq!(key_bytes(Key::Space, Modifiers::CTRL, false), Some(vec![0x00]));
        assert_eq!(key_bytes(Key::OpenBracket, Modifiers::CTRL, false), Some(vec![0x1b]));
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL | Modifiers::ALT, false), Some(vec![0x1b, 0x03]));
        // Ctrl+Shift+C is an application shortcut (copy); plain letters come as text.
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL | Modifiers::SHIFT, false), None);
        assert_eq!(key_bytes(Key::C, NONE, false), None);
    }

    #[test]
    fn cursor_and_function_keys() {
        assert_eq!(key_bytes(Key::ArrowUp, NONE, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(Key::ArrowLeft, Modifiers::CTRL, true).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes(Key::Home, NONE, false).unwrap(), b"\x1b[H");
        assert_eq!(key_bytes(Key::F1, NONE, false).unwrap(), b"\x1bOP");
        assert_eq!(key_bytes(Key::F5, NONE, false).unwrap(), b"\x1b[15~");
        assert_eq!(key_bytes(Key::Delete, Modifiers::SHIFT, false).unwrap(), b"\x1b[3;2~");
        assert_eq!(key_bytes(Key::Enter, NONE, false).unwrap(), b"\r");
        assert_eq!(key_bytes(Key::Backspace, NONE, false).unwrap(), b"\x7f");
        assert_eq!(key_bytes(Key::Tab, Modifiers::SHIFT, false).unwrap(), b"\x1b[Z");
        assert_eq!(key_bytes(Key::Escape, NONE, false).unwrap(), b"\x1b");
    }

    #[test]
    fn mouse_sgr_and_x10() {
        assert_eq!(mouse_report(true, BUTTON_LEFT, 4, 9, true).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(mouse_report(true, BUTTON_LEFT, 4, 9, false).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(mouse_report(true, WHEEL_UP, 0, 0, true).unwrap(), b"\x1b[<64;1;1M");
        let ctrl_right = BUTTON_RIGHT + mouse_modifiers(Modifiers::CTRL);
        assert_eq!(mouse_report(true, ctrl_right, 300, 1, true).unwrap(), b"\x1b[<18;301;2M");
        assert_eq!(mouse_report(false, BUTTON_LEFT, 4, 9, true).unwrap(), [0x1b, b'[', b'M', 32, 37, 42]);
        assert_eq!(mouse_report(false, BUTTON_RIGHT, 4, 9, false).unwrap(), [0x1b, b'[', b'M', 35, 37, 42]);
        assert_eq!(mouse_report(false, MOTION + BUTTON_LEFT, 0, 0, true).unwrap(), [0x1b, b'[', b'M', 64, 33, 33]);
        assert_eq!(mouse_report(false, BUTTON_LEFT, 223, 0, true), None);
    }

    #[test]
    fn paste_encoding() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
