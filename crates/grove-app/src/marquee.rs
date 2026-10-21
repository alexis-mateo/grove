//! Texte sur une ligne : tronqué avec « … » s'il déborde, et qui défile au survol
//! pour se lire en entier.

use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Pixels, Refineable, ShapedLine, SharedString, Style,
    StyleRefinement, Styled, Window, point, px,
};

/// Vitesse de défilement.
const SPEED: f32 = 45.0;
/// Pause avant de défiler, puis en fin de texte.
const PAUSE_START: Duration = Duration::from_millis(600);
const PAUSE_END: Duration = Duration::from_millis(1200);
/// Marge après le dernier caractère, pour qu'il soit entièrement visible.
const END_PADDING: f32 = 6.0;

pub struct Marquee {
    id: ElementId,
    text: SharedString,
    style: StyleRefinement,
}

/// `id` doit être unique parmi ses voisins : il porte l'instant de début du survol.
pub fn marquee(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Marquee {
    Marquee { id: id.into(), text: text.into(), style: StyleRefinement::default() }
}

pub struct Prepared {
    line: ShapedLine,
    /// Version tronquée, si le texte déborde.
    truncated: Option<ShapedLine>,
    overflow: Pixels,
    line_height: Pixels,
    hitbox: Hitbox,
}

/// Décalage horizontal après `elapsed` de survol : pause, défilement, pause, et on recommence.
fn offset(elapsed: Duration, overflow: f32) -> f32 {
    let travel = overflow / SPEED;
    let cycle = PAUSE_START.as_secs_f32() + travel + PAUSE_END.as_secs_f32();
    let t = elapsed.as_secs_f32() % cycle - PAUSE_START.as_secs_f32();
    (t * SPEED).clamp(0.0, overflow)
}

impl IntoElement for Marquee {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Styled for Marquee {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for Marquee {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepared>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let text_style = window.text_style();
        let line_height = text_style.line_height_in_pixels(window.rem_size());
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let width = window
            .text_system()
            .shape_line(self.text.clone(), font_size, &[text_style.to_run(self.text.len())], None)
            .width;
        let mut style = Style::default();
        // Largeur du texte (prise en compte par un parent qui s'ajuste à son contenu,
        // comme un onglet), puis comme un enfant flex : s'étire s'il y a de la place
        // pour pousser ce qui suit à droite, se réduit jusqu'à zéro sinon.
        style.size.width = width.ceil().into();
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.min_size.width = px(0.0).into();
        style.size.height = line_height.into();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Option<Prepared> {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = text_style.line_height_in_pixels(window.rem_size());
        let run = text_style.to_run(self.text.len());
        let line = window.text_system().shape_line(self.text.clone(), font_size, &[run.clone()], None);
        let overflow = line.width - bounds.size.width;

        let truncated = (overflow > px(0.0)).then(|| {
            let mut runs = vec![run];
            let text = window.text_system().line_wrapper(text_style.font(), font_size).truncate_line(
                self.text.clone(),
                bounds.size.width,
                "…",
                &mut runs,
            );
            window.text_system().shape_line(text, font_size, &runs, None)
        });
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Some(Prepared { line, truncated, overflow, line_height, hitbox })
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepared: &mut Option<Prepared>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(prepared) = prepared.take() else { return };
        let origin = point(bounds.origin.x, bounds.origin.y + (bounds.size.height - prepared.line_height) / 2.0);
        let scrolling = prepared.truncated.is_some() && prepared.hitbox.is_hovered(window);

        let Some(id) = id else { return };
        let hover_start = window.with_element_state::<Option<Instant>, _>(id, |state, _| {
            let start = scrolling.then(|| state.flatten().unwrap_or_else(Instant::now));
            (start, start)
        });

        match (hover_start, &prepared.truncated) {
            (Some(start), _) => {
                let shift = offset(start.elapsed(), f32::from(prepared.overflow) + END_PADDING);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let _ = prepared.line.paint(point(origin.x - px(shift), origin.y), prepared.line_height, window, cx);
                });
                window.request_animation_frame();
            }
            (None, Some(truncated)) => {
                let _ = truncated.paint(origin, prepared.line_height, window, cx);
            }
            (None, None) => {
                let _ = prepared.line.paint(origin, prepared.line_height, window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_scrolls_then_holds_the_end() {
        let at = |ms| offset(Duration::from_millis(ms), 90.0);
        assert_eq!(at(0), 0.0);
        assert_eq!(at(500), 0.0);
        assert!((at(1600) - 45.0).abs() < 0.01);
        // 90 px à 45 px/s : fin atteinte à 2,6 s, tenue pendant la pause de fin.
        assert_eq!(at(3000), 90.0);
        // Cycle de 0,6 + 2 + 1,2 = 3,8 s : on repart du début.
        assert_eq!(at(3900), 0.0);
    }
}
