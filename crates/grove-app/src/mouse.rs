//! Reporting souris xterm (modes 1000/1002/1003, encodages normal, UTF-8 et SGR).

use gpui::{Modifiers, MouseButton};
use grove_term::TermMode;

/// Bouton xterm : 0 gauche, 1 milieu, 2 droit, 64/65 molette haut/bas.
pub fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;
/// Ajouté au code du bouton pour un mouvement.
pub const MOTION: u8 = 32;
/// Mouvement sans bouton enfoncé (mode 1003).
pub const NO_BUTTON: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
}

/// Séquence pour un événement en (`column`, `row`), coordonnées 0-based.
/// `None` si les coordonnées ne sont pas représentables dans l'encodage actif.
pub fn encode(
    code: u8,
    action: Action,
    column: usize,
    row: usize,
    modifiers: &Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mods = 4 * modifiers.shift as u8 + 8 * modifiers.alt as u8 + 16 * modifiers.control as u8;
    let (x, y) = (column + 1, row + 1);

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if action == Action::Press { 'M' } else { 'm' };
        return Some(format!("\x1b[<{};{x};{y}{suffix}", code + mods).into_bytes());
    }

    // Encodages historiques : pas de bouton au relâchement, seulement « 3 ».
    let code = if action == Action::Release && code < 64 { NO_BUTTON } else { code };
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + code + mods);
    for v in [x, y] {
        let v = v + 32;
        if mode.contains(TermMode::UTF8_MOUSE) {
            let c = char::from_u32(v as u32).filter(|_| v <= 2047)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        } else {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_wheel_and_clicks() {
        let none = Modifiers::default();
        let sgr = TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(encode(WHEEL_UP, Action::Press, 4, 9, &none, sgr).unwrap(), b"\x1b[<64;5;10M");
        assert_eq!(encode(0, Action::Release, 0, 0, &none, sgr).unwrap(), b"\x1b[<0;1;1m");
        let ctrl = Modifiers { control: true, ..Default::default() };
        assert_eq!(encode(WHEEL_DOWN, Action::Press, 0, 0, &ctrl, sgr).unwrap(), b"\x1b[<81;1;1M");
    }

    #[test]
    fn normal_encoding() {
        let none = Modifiers::default();
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(encode(0, Action::Press, 0, 0, &none, mode).unwrap(), b"\x1b[M !!");
        assert_eq!(encode(2, Action::Release, 0, 0, &none, mode).unwrap(), b"\x1b[M#!!");
        assert_eq!(encode(0, Action::Press, 300, 0, &none, mode), None);
    }
}
