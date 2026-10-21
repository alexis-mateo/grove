//! Couleurs de l'interface (tons forêt, cf. specs §1.2).

use gpui::{Hsla, Rgba};

pub fn rgb(hex: u32) -> Hsla {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// Fond du terminal et de la zone principale.
pub fn background() -> Hsla {
    rgb(0x141917)
}
/// Barre de titre, sidebar, barre d'onglets.
pub fn panel() -> Hsla {
    rgb(0x1b211e)
}
pub fn hover() -> Hsla {
    rgb(0x252d29)
}
pub fn selected() -> Hsla {
    rgb(0x2c3631)
}
pub fn border() -> Hsla {
    rgb(0x2c3531)
}
pub fn text() -> Hsla {
    rgb(0xd8ddd3)
}
pub fn text_muted() -> Hsla {
    rgb(0x8a948e)
}
/// Vert mousse : état actif, accents.
pub fn accent() -> Hsla {
    rgb(0xa9d685)
}
/// Ambre : session qui attend une réponse.
pub fn waiting() -> Hsla {
    rgb(0xebcb84)
}
/// Rouge : limite proche, action destructive.
pub fn danger() -> Hsla {
    rgb(0xe06c62)
}
