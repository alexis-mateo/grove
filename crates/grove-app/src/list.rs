//! Listes longues des panneaux flottants (branches…) : rendu virtualisé
//! (`uniform_list`, seules les lignes visibles sont construites) et barre de
//! défilement déplaçable.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    InteractiveElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, UniformListScrollHandle,
    div, point, prelude::*, px,
};

use crate::theme;

pub const ROW_HEIGHT: Pixels = px(26.0);
/// Hauteur maximale de la liste ; au-delà, elle défile.
const MAX_HEIGHT: Pixels = px(338.0);
const MIN_THUMB: f32 = 24.0;

#[derive(Clone, Default)]
pub struct ListScroll {
    pub handle: UniformListScrollHandle,
    /// Position du pointeur dans le pouce au début du glisser, en pixels.
    drag: Rc<Cell<Option<f32>>>,
}

/// Géométrie de la barre, en pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Thumb {
    viewport: f32,
    content: f32,
    height: f32,
    top: f32,
}

impl Thumb {
    fn new(count: usize, scroll_top: f32) -> Option<Self> {
        let content = count as f32 * f32::from(ROW_HEIGHT);
        let viewport = content.min(f32::from(MAX_HEIGHT));
        if content <= viewport {
            return None;
        }
        let height = (viewport * viewport / content).max(MIN_THUMB);
        let range = content - viewport;
        let top = (scroll_top.clamp(0.0, range) / range) * (viewport - height);
        Some(Self { viewport, content, height, top })
    }

    /// Défilement correspondant à un pouce dont le haut est en `thumb_top`.
    fn scroll_for(&self, thumb_top: f32) -> f32 {
        let fraction = (thumb_top / (self.viewport - self.height)).clamp(0.0, 1.0);
        fraction * (self.content - self.viewport)
    }
}

impl ListScroll {
    /// Hauteur à donner à la liste de `count` lignes.
    pub fn height(count: usize) -> Pixels {
        (ROW_HEIGHT * count as f32).min(MAX_HEIGHT)
    }

    fn scroll_top(&self) -> f32 {
        -f32::from(self.handle.0.borrow().base_handle.offset().y)
    }

    fn set_scroll_top(&self, top: f32) {
        self.handle.0.borrow().base_handle.set_offset(point(px(0.0), px(-top)));
    }

    /// Fait défiler juste ce qu'il faut pour que la ligne `index` soit visible.
    pub fn reveal(&self, index: usize, count: usize) {
        let viewport = f32::from(Self::height(count));
        let (row_top, row) = (index as f32 * f32::from(ROW_HEIGHT), f32::from(ROW_HEIGHT));
        let top = self.scroll_top();
        if row_top < top {
            self.set_scroll_top(row_top);
        } else if row_top + row > top + viewport {
            self.set_scroll_top(row_top + row - viewport);
        }
    }

    pub fn reset(&self) {
        self.set_scroll_top(0.0);
    }

    /// Haut de la liste, en coordonnées de la fenêtre.
    fn list_top(&self) -> Pixels {
        self.handle.0.borrow().base_handle.bounds().top()
    }

    /// Barre de défilement, à placer dans un conteneur `relative` autour de la liste.
    /// `None` si tout tient sans défiler.
    pub fn scrollbar(&self, count: usize) -> Option<impl IntoElement> {
        let thumb = Thumb::new(count, self.scroll_top())?;
        let this = self.clone();
        Some(
            div()
                .id("list-scrollbar")
                .absolute()
                .top_0()
                .right_0()
                .w(px(10.0))
                .h(px(thumb.viewport))
                .cursor_default()
                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let y = f32::from(event.position.y - this.list_top());
                    let thumb = Thumb::new(count, this.scroll_top()).unwrap_or(thumb);
                    // Clic dans le pouce : on le saisit là ; ailleurs : il se centre sous le pointeur.
                    let grab = if (thumb.top..thumb.top + thumb.height).contains(&y) { y - thumb.top } else { thumb.height / 2.0 };
                    this.drag.set(Some(grab));
                    this.set_scroll_top(thumb.scroll_for(y - grab));
                    window.refresh();
                })
                .child(
                    div()
                        .absolute()
                        .top(px(thumb.top))
                        .right(px(2.0))
                        .w(px(6.0))
                        .h(px(thumb.height))
                        .rounded_full()
                        .bg(theme::selected())
                        .hover(|s| s.bg(theme::text_muted())),
                ),
        )
    }

    /// Suit le glisser du pouce : à poser sur le panneau qui contient la liste,
    /// pour continuer même quand le pointeur sort de la barre.
    pub fn track_drag<E: InteractiveElement>(&self, element: E, count: usize) -> E {
        let (moving, releasing) = (self.clone(), self.clone());
        element
            .on_mouse_move(move |event: &MouseMoveEvent, window, _| {
                let Some(grab) = moving.drag.get() else { return };
                if event.pressed_button != Some(MouseButton::Left) {
                    moving.drag.set(None);
                    return;
                }
                if let Some(thumb) = Thumb::new(count, moving.scroll_top()) {
                    let y = f32::from(event.position.y - moving.list_top());
                    moving.set_scroll_top(thumb.scroll_for(y - grab));
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| releasing.drag.set(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_scrollbar_when_everything_fits() {
        assert_eq!(Thumb::new(5, 0.0), None);
        assert_eq!(ListScroll::height(5), ROW_HEIGHT * 5.0);
    }

    #[test]
    fn thumb_follows_scroll_and_back() {
        // 100 lignes de 26 px dans 338 px : pouce de 43,9 px.
        let thumb = Thumb::new(100, 0.0).unwrap();
        assert_eq!(thumb.top, 0.0);
        let range = 100.0 * 26.0 - 338.0;
        let bottom = Thumb::new(100, range).unwrap();
        assert!((bottom.top + bottom.height - 338.0).abs() < 0.01);
        assert!((thumb.scroll_for(bottom.top) - range).abs() < 0.01);
        assert_eq!(thumb.scroll_for(-50.0), 0.0);
    }
}
