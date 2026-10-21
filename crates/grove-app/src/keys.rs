//! Traduction des frappes GPUI en séquences d'octets xterm.

use gpui::Keystroke;

/// Octets à envoyer au PTY pour `keystroke`, ou `None` si la touche n'a pas
/// de traduction (elle peut alors servir de raccourci à l'app).
///
/// `app_cursor` : mode « application cursor keys » (DECCKM), activé par les TUI.
pub fn to_esc(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    // Paramètre de modificateur xterm : 1 + shift + 2·alt + 4·ctrl.
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let modified = modifier > 1;

    let csi_letter = |letter: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |code: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let ss3 = |letter: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else {
            format!("\x1bO{letter}").into_bytes()
        }
    };

    let bytes = match keystroke.key.as_str() {
        "enter" => alt_prefixed(m.alt, b"\r"),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "backspace" if m.control => b"\x08".to_vec(),
        "backspace" => alt_prefixed(m.alt, b"\x7f"),
        "space" if m.control => b"\0".to_vec(),
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "insert" => csi_tilde(2),
        "delete" => csi_tilde(3),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        key if m.control && !m.platform => {
            let byte = control_byte(key)?;
            alt_prefixed(m.alt, &[byte])
        }
        _ if m.platform => return None,
        _ => {
            let text = keystroke.key_char.as_deref()?;
            alt_prefixed(m.alt, text.as_bytes())
        }
    };
    Some(bytes)
}

fn alt_prefixed(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

/// Ctrl + touche → caractère de contrôle C0.
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '/' | '7' => Some(0x1f),
        '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(s: &str) -> Option<Vec<u8>> {
        let mut keystroke = Keystroke::parse(s).unwrap();
        if keystroke.key_char.is_none()
            && keystroke.key.chars().count() == 1
            && !keystroke.modifiers.control
            && !keystroke.modifiers.platform
        {
            keystroke.key_char = Some(keystroke.key.clone());
        }
        to_esc(&keystroke, false)
    }

    #[test]
    fn printable_and_control() {
        assert_eq!(esc("a"), Some(b"a".to_vec()));
        assert_eq!(esc("ctrl-c"), Some(vec![0x03]));
        assert_eq!(esc("ctrl-["), Some(vec![0x1b]));
        assert_eq!(esc("alt-b"), Some(b"\x1bb".to_vec()));
        assert_eq!(esc("enter"), Some(b"\r".to_vec()));
        assert_eq!(esc("backspace"), Some(vec![0x7f]));
        assert_eq!(esc("shift-tab"), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn cursor_keys() {
        assert_eq!(esc("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(esc("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(esc("shift-delete"), Some(b"\x1b[3;2~".to_vec()));
        let up = Keystroke::parse("up").unwrap();
        assert_eq!(to_esc(&up, true), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn super_shortcuts_are_left_to_the_app() {
        assert_eq!(esc("cmd-v"), None);
    }
}
