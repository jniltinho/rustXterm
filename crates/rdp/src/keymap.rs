//! X11 keysym → PC/AT set-1 scancode (`0xE0xx` = extended) for a US layout, plus the
//! printable-character fallback sent as RDP unicode keyboard events.

/// Scancode for `keysym`, or `None` when the key has no US-layout position.
pub fn scancode(keysym: u32) -> Option<u16> {
    let ascii = u8::try_from(keysym).ok().filter(u8::is_ascii_graphic).map(|b| b.to_ascii_lowercase());
    if let Some(c) = ascii {
        return ascii_scancode(c);
    }
    Some(match keysym {
        0x0020 => 0x39,                                    // space
        0xff08 => 0x0e,                                    // BackSpace
        0xff09 | 0xfe20 => 0x0f,                           // Tab, ISO_Left_Tab
        0xff0d => 0x1c,                                    // Return
        0xff1b => 0x01,                                    // Escape
        0xff13 => 0x45,                                    // Pause (approximation)
        0xff14 => 0x46,                                    // Scroll_Lock
        0xff61 => 0xe037,                                  // Print
        0xff63 => 0xe052,                                  // Insert
        0xffff => 0xe053,                                  // Delete
        0xff50 => 0xe047,                                  // Home
        0xff57 => 0xe04f,                                  // End
        0xff55 => 0xe049,                                  // Prior
        0xff56 => 0xe051,                                  // Next
        0xff51 => 0xe04b,                                  // Left
        0xff52 => 0xe048,                                  // Up
        0xff53 => 0xe04d,                                  // Right
        0xff54 => 0xe050,                                  // Down
        0xff67 => 0xe05d,                                  // Menu
        0xffe1 => 0x2a,                                    // Shift_L
        0xffe2 => 0x36,                                    // Shift_R
        0xffe3 => 0x1d,                                    // Control_L
        0xffe4 => 0xe01d,                                  // Control_R
        0xffe5 => 0x3a,                                    // Caps_Lock
        0xffe9 | 0xffe7 => 0x38,                           // Alt_L, Meta_L
        0xffea | 0xffe8 | 0xfe03 => 0xe038,                // Alt_R, Meta_R, ISO_Level3_Shift (AltGr)
        0xffeb => 0xe05b,                                  // Super_L
        0xffec => 0xe05c,                                  // Super_R
        k @ 0xffbe..=0xffc7 => 0x3b + (k - 0xffbe) as u16, // F1..F10
        0xffc8 => 0x57,                                    // F11
        0xffc9 => 0x58,                                    // F12
        0xff7f => 0x45,                                    // Num_Lock
        0xff8d => 0xe01c,                                  // KP_Enter
        0xffaa => 0x37,                                    // KP_Multiply
        0xffab => 0x4e,                                    // KP_Add
        0xffad => 0x4a,                                    // KP_Subtract
        0xffae | 0xff9f => 0x53,                           // KP_Decimal, KP_Delete
        0xffaf => 0xe035,                                  // KP_Divide
        0xffb0 | 0xff9e => 0x52,                           // KP_0, KP_Insert
        0xffb1 | 0xff9c => 0x4f,                           // KP_1, KP_End
        0xffb2 | 0xff99 => 0x50,                           // KP_2, KP_Down
        0xffb3 | 0xff9b => 0x51,                           // KP_3, KP_Next
        0xffb4 | 0xff96 => 0x4b,                           // KP_4, KP_Left
        0xffb5 | 0xff9d => 0x4c,                           // KP_5, KP_Begin
        0xffb6 | 0xff98 => 0x4d,                           // KP_6, KP_Right
        0xffb7 | 0xff95 => 0x47,                           // KP_7, KP_Home
        0xffb8 | 0xff97 => 0x48,                           // KP_8, KP_Up
        0xffb9 | 0xff9a => 0x49,                           // KP_9, KP_Prior
        _ => return None,
    })
}

fn ascii_scancode(c: u8) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13,
        0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    Some(match c {
        b'a'..=b'z' => LETTERS[usize::from(c - b'a')],
        b'1'..=b'9' => 0x02 + u16::from(c - b'1'),
        b'0' | b')' => 0x0b,
        b'!' => 0x02,
        b'@' => 0x03,
        b'#' => 0x04,
        b'$' => 0x05,
        b'%' => 0x06,
        b'^' => 0x07,
        b'&' => 0x08,
        b'*' => 0x09,
        b'(' => 0x0a,
        b'-' | b'_' => 0x0c,
        b'=' | b'+' => 0x0d,
        b'[' | b'{' => 0x1a,
        b']' | b'}' => 0x1b,
        b';' | b':' => 0x27,
        b'\'' | b'"' => 0x28,
        b'`' | b'~' => 0x29,
        b'\\' | b'|' => 0x2b,
        b',' | b'<' => 0x33,
        b'.' | b'>' => 0x34,
        b'/' | b'?' => 0x35,
        _ => return None,
    })
}

/// Printable character for `keysym` (Latin-1 range or `0x0100_0000 | ucs`), for unicode events.
pub fn unicode(keysym: u32) -> Option<char> {
    let ucs = match keysym {
        0x20..=0x7e | 0xa0..=0xff => keysym,
        0x0100_0000..=0x0110_ffff => keysym - 0x0100_0000,
        _ => return None,
    };
    char::from_u32(ucs).filter(|c| !c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples() {
        assert_eq!(scancode(u32::from(b'a')), Some(0x1e));
        assert_eq!(scancode(u32::from(b'Z')), Some(0x2c));
        assert_eq!(scancode(u32::from(b'1')), Some(0x02));
        assert_eq!(scancode(u32::from(b'0')), Some(0x0b));
        assert_eq!(scancode(u32::from(b'?')), Some(0x35));
        assert_eq!(scancode(0xffbe), Some(0x3b)); // F1
        assert_eq!(scancode(0xffc7), Some(0x44)); // F10
        assert_eq!(scancode(0xffc9), Some(0x58)); // F12
        assert_eq!(scancode(0xffe4), Some(0xe01d)); // Control_R
        assert_eq!(scancode(0xff52), Some(0xe048)); // Up
        assert_eq!(scancode(0xffff), Some(0xe053)); // Delete
        assert_eq!(scancode(0xffb7), Some(0x47)); // KP_7
        assert_eq!(scancode(0xff8d), Some(0xe01c)); // KP_Enter
        assert_eq!(scancode(0x00e9), None); // eacute: no US key
        assert_eq!(unicode(0x00e9), Some('é'));
        assert_eq!(unicode(0x0100_20ac), Some('€'));
        assert_eq!(unicode(0xff0d), None);
    }
}
