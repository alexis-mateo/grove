//! Couleurs de l'interface : graphite neutre, accent émeraude (cf. specs §1.2).

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
    rgb(0x0e0f11)
}
/// Sidebar, panneaux latéraux.
pub fn panel() -> Hsla {
    rgb(0x131417)
}
/// Panneaux flottants (sélecteurs, dialogues, infobulles).
pub fn elevated() -> Hsla {
    rgb(0x1a1b1f)
}
pub fn hover() -> Hsla {
    rgb(0x1d1f23)
}
pub fn selected() -> Hsla {
    rgb(0x25272c)
}
pub fn border() -> Hsla {
    rgb(0x212328)
}
/// Bordure des éléments en relief (onglet actif, champs, boutons).
pub fn border_strong() -> Hsla {
    rgb(0x2f3238)
}
pub fn text() -> Hsla {
    rgb(0xe6e7ea)
}
pub fn text_muted() -> Hsla {
    rgb(0x9196a0)
}
/// Texte secondaire discret : compteurs, raccourcis, libellés de section.
pub fn text_faint() -> Hsla {
    rgb(0x5f636d)
}
/// Émeraude : état actif, accents.
pub fn accent() -> Hsla {
    rgb(0x4fd18b)
}
/// Bleu : liens, distincts du code (accent) dans le texte.
pub fn link() -> Hsla {
    rgb(0x6cb6ff)
}
/// Texte posé sur l'accent (bouton principal).
pub fn on_accent() -> Hsla {
    rgb(0x07150d)
}
/// Ambre : session qui attend une réponse.
pub fn waiting() -> Hsla {
    rgb(0xf0b44c)
}
/// Rouge : limite proche, action destructive.
pub fn danger() -> Hsla {
    rgb(0xef5b5b)
}
