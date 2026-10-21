use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// Palette de base : 16 couleurs ANSI + premier plan, fond et curseur.
/// Les couleurs 16–255 sont calculées (cube 6×6×6 et niveaux de gris).
#[derive(Clone, Debug)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
}

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

impl Default for Palette {
    /// Thème sombre graphite, accordé à l'interface (`grove-app/src/theme.rs`).
    fn default() -> Self {
        Self {
            ansi: [
                rgb(0x23252b), rgb(0xef6b6b), rgb(0x5fd394), rgb(0xe8b95c),
                rgb(0x62a8f0), rgb(0xc08cf0), rgb(0x4fc4c4), rgb(0xc8cbd2),
                rgb(0x5d616d), rgb(0xff8a8a), rgb(0x84e2ae), rgb(0xf4cf80),
                rgb(0x8cc0ff), rgb(0xd6aaff), rgb(0x7adada), rgb(0xf2f3f5),
            ],
            foreground: rgb(0xe2e4e8),
            background: rgb(0x0e0f11),
            cursor: rgb(0x4fd18b),
        }
    }
}

impl Palette {
    /// Couleur RGB d'un index 0–255.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            232..=255 => {
                let v = (index - 232) * 10 + 8;
                Rgb { r: v, g: v, b: v }
            }
        }
    }

    fn named(&self, color: NamedColor) -> Rgb {
        match color {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => dim(self.foreground),
            c if (c as usize) < 16 => self.ansi[c as usize],
            // Dim* : couleur normale correspondante, assombrie.
            c => dim(self.ansi[c as usize - NamedColor::DimBlack as usize]),
        }
    }

    /// Résout une couleur de cellule, en tenant compte des couleurs redéfinies
    /// par le programme (OSC 4/10/11) et des attributs gras / atténué.
    pub(crate) fn resolve(&self, color: Color, overrides: &Colors, flags: Flags) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Indexed(i) => {
                // Gras sur une couleur 0–7 : version claire, comme la plupart des terminaux.
                let i = if flags.contains(Flags::BOLD) && i < 8 { i + 8 } else { i };
                let rgb = overrides[i as usize].unwrap_or_else(|| self.indexed(i));
                if flags.contains(Flags::DIM) { dim(rgb) } else { rgb }
            }
            Color::Named(mut named) => {
                if flags.contains(Flags::DIM) {
                    named = named.to_dim();
                } else if flags.contains(Flags::BOLD) {
                    named = named.to_bright();
                }
                overrides[named].unwrap_or_else(|| self.named(named))
            }
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    let f = |v: u8| (v as u16 * 2 / 3) as u8;
    Rgb { r: f(c.r), g: f(c.g), b: f(c.b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grayscale() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(p.indexed(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(p.indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(p.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(p.indexed(255), Rgb { r: 238, g: 238, b: 238 });
    }

    #[test]
    fn bold_brightens_and_overrides_win() {
        let p = Palette::default();
        let mut overrides = Colors::default();
        let red = Color::Named(NamedColor::Red);
        assert_eq!(p.resolve(red, &overrides, Flags::BOLD), p.ansi[9]);

        overrides[NamedColor::Red] = Some(rgb(0x123456));
        assert_eq!(p.resolve(red, &overrides, Flags::empty()), rgb(0x123456));
    }
}
