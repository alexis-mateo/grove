//! Cadre de la fenêtre : décorations côté client (Wayland sans SSD, comme GNOME).
//!
//! Pas de barre de titre séparée : la sidebar et la barre d'onglets forment la
//! bande du haut, qui sert de poignée (`drag_region`) et porte les boutons de
//! la fenêtre (`window_controls`).

use gpui::{
    AnyElement, Bounds, BoxShadow, CursorStyle, Decorations, Hsla, HitboxBehavior, MouseButton, Pixels,
    Point, ResizeEdge, Size, Window, canvas, div, point, prelude::*, px, transparent_black,
};

use crate::theme::{self, rgb};
use crate::ui::{Icon, sized_icon};

const SHADOW: Pixels = px(10.0);
const ROUNDING: Pixels = px(10.0);
/// Hauteur de la bande du haut (en-tête de la sidebar, barre d'onglets).
pub const TOP_BAR_HEIGHT: Pixels = px(40.0);

/// Enveloppe `content` et, si le compositeur ne dessine pas les décorations,
/// lui ajoute une ombre et des bords redimensionnables.
pub fn frame(content: AnyElement, window: &mut Window) -> AnyElement {
    let decorations = window.window_decorations();
    let client = matches!(decorations, Decorations::Client { .. });
    window.set_client_inset(if client { SHADOW } else { px(0.0) });

    let inner = div()
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme::background())
        .cursor(CursorStyle::Arrow)
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        // Un clic dans le contenu ne doit jamais redimensionner la fenêtre.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().flex_1().min_h_0().child(content));

    let Decorations::Client { tiling } = decorations else {
        return inner.into_any_element();
    };

    let inner = inner
        .border_color(theme::border())
        .when(!(tiling.top || tiling.left), |d| d.rounded_tl(ROUNDING))
        .when(!(tiling.top || tiling.right), |d| d.rounded_tr(ROUNDING))
        .when(!(tiling.bottom || tiling.left), |d| d.rounded_bl(ROUNDING))
        .when(!(tiling.bottom || tiling.right), |d| d.rounded_br(ROUNDING))
        .when(!tiling.top, |d| d.border_t_1())
        .when(!tiling.bottom, |d| d.border_b_1())
        .when(!tiling.left, |d| d.border_l_1())
        .when(!tiling.right, |d| d.border_r_1())
        .when(!tiling.is_tiled(), |d| {
            d.shadow(vec![BoxShadow {
                color: Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.4 },
                blur_radius: SHADOW / 2.0,
                spread_radius: px(0.0),
                offset: point(px(0.0), px(0.0)),
            }])
        });

    div()
        .id("window-backdrop")
        .size_full()
        .bg(transparent_black())
        .child(
            // Curseur de redimensionnement au survol des bords.
            canvas(
                |_, window, _| {
                    window.insert_hitbox(
                        Bounds::new(point(px(0.0), px(0.0)), window.viewport_size()),
                        HitboxBehavior::Normal,
                    )
                },
                |_, hitbox, window, _| {
                    let size = window.viewport_size();
                    if let Some(edge) = resize_edge(window.mouse_position(), size) {
                        window.set_cursor_style(edge_cursor(edge), &hitbox);
                    }
                },
            )
            .size_full()
            .absolute(),
        )
        .when(!tiling.top, |d| d.pt(SHADOW))
        .when(!tiling.bottom, |d| d.pb(SHADOW))
        .when(!tiling.left, |d| d.pl(SHADOW))
        .when(!tiling.right, |d| d.pr(SHADOW))
        .on_mouse_move(|_, window, _| window.refresh())
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            let size = window.viewport_size();
            if let Some(edge) = resize_edge(event.position, size) {
                window.start_window_resize(edge);
            }
        })
        .child(inner)
        .into_any_element()
}

/// Zone vide d'une barre du haut : la saisir déplace la fenêtre, un double-clic
/// l'agrandit, un clic droit ouvre le menu de la fenêtre. À poser sur un espace
/// sans boutons : les clics des enfants remontent jusqu'ici.
pub fn drag_region<E: InteractiveElement>(element: E) -> E {
    element
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, _| window.show_window_menu(event.position))
}

/// Boutons réduire / agrandir / fermer, si le compositeur ne les dessine pas.
pub fn window_controls(window: &Window) -> Option<impl IntoElement + use<>> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }
    let controls = window.window_controls();
    let button = |id: &'static str, glyph: Icon, close: bool| {
        div()
            .id(id)
            .group(id)
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .hover(move |s| s.bg(if close { theme::danger().opacity(0.9) } else { theme::selected() }))
            .cursor_pointer()
            // Le clic sur un bouton ne doit pas déclencher le déplacement de la fenêtre.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                sized_icon(glyph, px(14.0), theme::text_muted())
                    .group_hover(id, move |s| s.text_color(if close { rgb(0xffffff) } else { theme::text() })),
            )
    };
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl_1()
            .pr_2()
            .when(controls.minimize, |d| {
                d.child(button("minimize", Icon::WindowMinimize, false).on_click(|_, window, _| window.minimize_window()))
            })
            .when(controls.maximize, |d| {
                d.child(button("maximize", Icon::WindowMaximize, false).on_click(|_, window, _| window.zoom_window()))
            })
            .child(button("close", Icon::WindowClose, true).on_click(|_, window, _| window.remove_window())),
    )
}

/// Bord sous `pos`. `size` : surface entière (`viewport_size`), ombre comprise ;
/// `window_bounds` l'exclut et décalerait la zone dans le contenu.
fn resize_edge(pos: Point<Pixels>, size: Size<Pixels>) -> Option<ResizeEdge> {
    let (top, bottom) = (pos.y < SHADOW, pos.y > size.height - SHADOW);
    let (left, right) = (pos.x < SHADOW, pos.x > size.width - SHADOW);
    Some(match (top, bottom, left, right) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (true, _, _, true) => ResizeEdge::TopRight,
        (_, true, true, _) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (true, ..) => ResizeEdge::Top,
        (_, true, ..) => ResizeEdge::Bottom,
        (_, _, true, _) => ResizeEdge::Left,
        (_, _, _, true) => ResizeEdge::Right,
        _ => return None,
    })
}

fn edge_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}
