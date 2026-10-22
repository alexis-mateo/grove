//! Caractères dessinés en rectangles plutôt qu'avec la police : éléments de bloc
//! (U+2580–259F) et traits de dessin de boîtes simples (U+2500–257F).
//!
//! La police ne remplit pas la hauteur d'une ligne (interligne de 1,3) : sans
//! ce traitement, logos en blocs et cadres présentent des trous entre les lignes.

/// Rectangle en pixels physiques, relatif au coin haut-gauche de la cellule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Glyph {
    pub rects: Vec<Rect>,
    /// Opacité du remplissage (< 1 pour les trames ░▒▓).
    pub alpha: f32,
}

/// Géométrie de `c` pour une cellule de `w`×`h` pixels physiques (entiers), ou
/// `None` si la police s'en charge. `stroke` : épaisseur d'un trait fin.
pub fn glyph(c: char, w: f32, h: f32, stroke: f32) -> Option<Glyph> {
    block(c, w, h).or_else(|| line(c, w, h, stroke))
}

fn block(c: char, w: f32, h: f32) -> Option<Glyph> {
    let frac = |x: f32, y: f32, fw: f32, fh: f32| snap(Rect { x: x * w, y: y * h, w: fw * w, h: fh * h });
    let quadrants = |ul: bool, ur: bool, ll: bool, lr: bool| {
        [(ul, 0.0, 0.0), (ur, 0.5, 0.0), (ll, 0.0, 0.5), (lr, 0.5, 0.5)]
            .into_iter()
            .filter(|q| q.0)
            .map(|(_, x, y)| frac(x, y, 0.5, 0.5))
            .collect()
    };
    let solid = |rects: Vec<Rect>| Some(Glyph { rects, alpha: 1.0 });
    let shade = |alpha: f32| Some(Glyph { rects: vec![frac(0.0, 0.0, 1.0, 1.0)], alpha });

    let code = c as u32;
    match c {
        '\u{2580}' => solid(vec![frac(0.0, 0.0, 1.0, 0.5)]),
        // ▁ … ▇ : n/8 bas ; █ : plein.
        '\u{2581}'..='\u{2588}' => {
            let n = (code - 0x2580) as f32 / 8.0;
            solid(vec![frac(0.0, 1.0 - n, 1.0, n)])
        }
        // ▉ … ▏ : 7/8 … 1/8 gauche.
        '\u{2589}'..='\u{258f}' => solid(vec![frac(0.0, 0.0, (0x2590 - code) as f32 / 8.0, 1.0)]),
        '\u{2590}' => solid(vec![frac(0.5, 0.0, 0.5, 1.0)]),
        '\u{2591}' => shade(0.25),
        '\u{2592}' => shade(0.5),
        '\u{2593}' => shade(0.75),
        '\u{2594}' => solid(vec![frac(0.0, 0.0, 1.0, 0.125)]),
        '\u{2595}' => solid(vec![frac(0.875, 0.0, 0.125, 1.0)]),
        '\u{2596}' => solid(quadrants(false, false, true, false)),
        '\u{2597}' => solid(quadrants(false, false, false, true)),
        '\u{2598}' => solid(quadrants(true, false, false, false)),
        '\u{2599}' => solid(quadrants(true, false, true, true)),
        '\u{259a}' => solid(quadrants(true, false, false, true)),
        '\u{259b}' => solid(quadrants(true, true, true, false)),
        '\u{259c}' => solid(quadrants(true, true, false, true)),
        '\u{259d}' => solid(quadrants(false, true, false, false)),
        '\u{259e}' => solid(quadrants(false, true, true, false)),
        '\u{259f}' => solid(quadrants(false, true, true, true)),
        _ => None,
    }
}

/// Branches (haut, bas, gauche, droite) d'un trait : 0 absent, 1 fin, 2 épais.
fn arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' | '┄' | '┈' | '╌' => [0, 0, 1, 1],
        '━' | '┅' | '┉' | '╍' => [0, 0, 2, 2],
        '│' | '┆' | '┊' | '╎' => [1, 1, 0, 0],
        '┃' | '┇' | '┋' | '╏' => [2, 2, 0, 0],
        '┌' | '╭' => [0, 1, 0, 1],
        '┏' => [0, 2, 0, 2],
        '┐' | '╮' => [0, 1, 1, 0],
        '┓' => [0, 2, 2, 0],
        '└' | '╰' => [1, 0, 0, 1],
        '┗' => [2, 0, 0, 2],
        '┘' | '╯' => [1, 0, 1, 0],
        '┛' => [2, 0, 2, 0],
        '├' => [1, 1, 0, 1],
        '┣' => [2, 2, 0, 2],
        '┤' => [1, 1, 1, 0],
        '┫' => [2, 2, 2, 0],
        '┬' => [0, 1, 1, 1],
        '┳' => [0, 2, 2, 2],
        '┴' => [1, 0, 1, 1],
        '┻' => [2, 0, 2, 2],
        '┼' => [1, 1, 1, 1],
        '╋' => [2, 2, 2, 2],
        '╴' => [0, 0, 1, 0],
        '╵' => [1, 0, 0, 0],
        '╶' => [0, 0, 0, 1],
        '╷' => [0, 1, 0, 0],
        '╸' => [0, 0, 2, 0],
        '╹' => [2, 0, 0, 0],
        '╺' => [0, 0, 0, 2],
        '╻' => [0, 2, 0, 0],
        _ => return None,
    })
}

fn line(c: char, w: f32, h: f32, stroke: f32) -> Option<Glyph> {
    let [up, down, left, right] = arms(c)?;
    let thickness = |weight: u8| (stroke * weight as f32).round().max(1.0);
    // Centre du trait, aligné sur le pixel pour rester net.
    let cx = ((w - stroke) / 2.0).round();
    let cy = ((h - stroke) / 2.0).round();
    // Les branches se prolongent jusqu'au bord opposé du trait le plus épais
    // pour que les angles se rejoignent.
    let vertical = thickness(up.max(down));
    let horizontal = thickness(left.max(right));

    let mut rects = Vec::new();
    if up > 0 {
        let t = thickness(up);
        rects.push(Rect { x: cx, y: 0.0, w: t, h: cy + horizontal });
    }
    if down > 0 {
        let t = thickness(down);
        rects.push(Rect { x: cx, y: cy, w: t, h: h - cy });
    }
    if left > 0 {
        let t = thickness(left);
        rects.push(Rect { x: 0.0, y: cy, w: cx + vertical, h: t });
    }
    if right > 0 {
        let t = thickness(right);
        rects.push(Rect { x: cx, y: cy, w: w - cx, h: t });
    }
    Some(Glyph { rects, alpha: 1.0 })
}

/// Arrondit au pixel : deux blocs voisins se touchent sans trou ni chevauchement.
fn snap(r: Rect) -> Rect {
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.w).round(), (r.y + r.h).round());
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_block_fills_the_cell() {
        let g = glyph('█', 9.0, 18.0, 1.0).unwrap();
        assert_eq!(g.rects, [Rect { x: 0.0, y: 0.0, w: 9.0, h: 18.0 }]);
    }

    #[test]
    fn quadrants_cover_without_gaps() {
        // ▛ + ▗ = cellule pleine, même avec une largeur impaire.
        let area = |c| glyph(c, 9.0, 18.0, 1.0).unwrap().rects.iter().map(|r| r.w * r.h).sum::<f32>();
        assert_eq!(area('▛') + area('▗'), 9.0 * 18.0);
    }

    #[test]
    fn horizontal_line_spans_the_cell() {
        let g = glyph('─', 9.0, 18.0, 1.0).unwrap();
        let start = g.rects.iter().map(|r| r.x).fold(f32::MAX, f32::min);
        let end = g.rects.iter().map(|r| r.x + r.w).fold(0.0, f32::max);
        assert_eq!((start, end), (0.0, 9.0));
        assert!(g.rects.iter().all(|r| r.h == 1.0));
    }

    #[test]
    fn letters_use_the_font() {
        assert!(glyph('a', 9.0, 18.0, 1.0).is_none());
    }
}
