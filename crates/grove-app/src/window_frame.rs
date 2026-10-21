//! Cadre de la fenêtre : décorations côté client (Wayland sans SSD, comme GNOME).

use gpui::{
    AnyElement, App, Bounds, BoxShadow, CursorStyle, Decorations, Hsla, HitboxBehavior,
    MouseButton, Pixels, Point, ResizeEdge, SharedString, Size, Window, canvas, div, point,
    prelude::*, px, transparent_black,
};

use crate::theme::{self, rgb};

const SHADOW: Pixels = px(10.0);
const ROUNDING: Pixels = px(8.0);
pub const TITLEBAR_HEIGHT: Pixels = px(32.0);

/// Enveloppe `content` avec une barre de titre et, si le compositeur ne
/// dessine pas les décorations, une ombre et des bords redimensionnables.
pub fn frame(title: SharedString, content: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
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
        .when(client, |div| div.child(titlebar(title, window, cx)))
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

fn titlebar(title: SharedString, window: &mut Window, _: &mut App) -> impl IntoElement {
    let controls = window.window_controls();
    let button = |id: &'static str, label: &'static str, hover: u32| {
        div()
            .id(id)
            .w(px(36.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x9aa59e))
            .hover(move |s| s.bg(rgb(hover)).text_color(rgb(0xeef1ea)))
            .cursor_pointer()
            // Le clic sur un bouton ne doit pas déclencher le déplacement de la fenêtre.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(label)
    };

    div()
        .id("titlebar")
        .h(TITLEBAR_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .bg(theme::panel())
        .border_b_1()
        .border_color(theme::border())
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, _| window.show_window_menu(event.position))
        .child(
            div()
                .flex_1()
                .px_3()
                .text_sm()
                .text_color(rgb(0xc9cfc6))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(title),
        )
        .when(controls.minimize, |d| {
            d.child(button("minimize", "–", 0x2c3531).on_click(|_, window, _| window.minimize_window()))
        })
        .when(controls.maximize, |d| {
            d.child(button("maximize", "□", 0x2c3531).on_click(|_, window, _| window.zoom_window()))
        })
        .child(button("close", "×", 0xb5473f).on_click(|_, window, _| window.remove_window()))
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
