//! Briques d'interface partagées : icônes, boutons, infobulles, raccourcis.

use std::borrow::Cow;

use gpui::{
    AnyView, App, AssetSource, Context, Div, Hsla, Pixels, Render, SharedString, Stateful, Svg, Window, div,
    prelude::*, px, svg,
};

use crate::text_field::TextField;
use crate::theme;

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($variant),* }

        impl Icon {
            fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file, ".svg")),* }
            }
        }

        /// Icônes embarquées dans le binaire (`assets/icons`).
        pub struct Assets;

        impl AssetSource for Assets {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/", $file, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../assets/icons/", $file, ".svg")).as_slice(),
                    )),)*
                    _ => None,
                })
            }

            fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
                Ok([$(concat!("icons/", $file, ".svg")),*]
                    .into_iter()
                    .filter(|p| p.starts_with(path))
                    .map(SharedString::from)
                    .collect())
            }
        }
    };
}

icons! {
    Alert => "alert",
    ArrowDown => "arrow-down",
    ArrowUp => "arrow-up",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    Claude => "claude",
    Copy => "copy",
    CloudUpload => "cloud-upload",
    CornerDownRight => "corner-down-right",
    Folder => "folder",
    FolderPlus => "folder-plus",
    GitBranch => "git-branch",
    History => "history",
    House => "house",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    Plus => "plus",
    Refresh => "refresh",
    Search => "search",
    Square => "square",
    Terminal => "terminal",
    Trash => "trash",
    WindowClose => "window-close",
    WindowMaximize => "window-maximize",
    WindowMinimize => "window-minimize",
    X => "x",
}

/// Icône de 14 px. Le SVG n'hérite pas de la couleur du texte : on la donne ici.
pub fn icon(icon: Icon, color: Hsla) -> Svg {
    sized_icon(icon, px(14.0), color)
}

pub fn sized_icon(icon: Icon, size: Pixels, color: Hsla) -> Svg {
    svg().path(icon.path()).size(size).flex_none().text_color(color)
}

/// Bouton carré ne contenant qu'une icône. `id` sert aussi de groupe de survol.
pub fn icon_button(id: impl Into<SharedString>, glyph: Icon) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(id.clone())
        .group(id.clone())
        .size(px(24.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .hover(|s| s.bg(theme::selected()))
        .cursor_pointer()
        .child(icon(glyph, theme::text_muted()).group_hover(id, |s| s.text_color(theme::text())))
}

/// Variante d'un bouton de texte.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Plein, couleur d'accent.
    Primary,
    /// Contour discret.
    Secondary,
    /// Teinté de rouge : action irréversible.
    Danger,
}

/// Bouton de texte, avec une icône facultative devant.
pub fn button(id: impl Into<SharedString>, label: impl Into<SharedString>, style: ButtonStyle) -> Stateful<Div> {
    let id: SharedString = id.into();
    let base = div()
        .id(id)
        .h(px(26.0))
        .px_2p5()
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .rounded_md()
        .text_sm()
        .whitespace_nowrap()
        .cursor_pointer();
    match style {
        ButtonStyle::Primary => base
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(gpui::FontWeight::MEDIUM)
            .hover(|s| s.bg(theme::accent().opacity(0.85))),
        ButtonStyle::Danger => base
            .bg(theme::danger().opacity(0.16))
            .border_1()
            .border_color(theme::danger().opacity(0.4))
            .text_color(theme::danger())
            .hover(|s| s.bg(theme::danger().opacity(0.26))),
        ButtonStyle::Secondary => base
            .border_1()
            .border_color(theme::border_strong())
            .text_color(theme::text())
            .hover(|s| s.bg(theme::hover())),
    }
    .child(label.into())
}

/// Touche de raccourci : « Échap », « Ctrl+Shift+T ».
pub fn kbd(keys: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::hover())
        .text_size(px(11.0))
        .text_color(theme::text_muted())
        .child(keys.into())
}

/// Séparateur vertical, pour les barres.
pub fn vertical_divider() -> Div {
    div().flex_none().w(px(1.0)).h(px(16.0)).mx_1().bg(theme::border_strong())
}

/// Pastille d'état ronde.
pub fn dot(color: Hsla) -> Div {
    div().flex_none().size(px(6.0)).rounded_full().bg(color)
}

/// Infobulle : à passer à `.tooltip(…)`.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Décalée du pointeur pour ne pas le masquer.
        div().pl_2().pt_4().child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::elevated())
                .shadow_md()
                .text_xs()
                .text_color(theme::text())
                .child(self.text.clone()),
        )
    }
}

/// Panneau flottant au-dessus du terminal, qui capture clavier et souris.
pub fn popover(id: &'static str, focus: &gpui::FocusHandle, width: Pixels) -> Stateful<Div> {
    div()
        .id(id)
        .track_focus(focus)
        .occlude()
        .absolute()
        .top(px(6.0))
        .left(px(8.0))
        .w(width)
        .max_h(px(480.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded_lg()
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::elevated())
        .shadow_lg()
        .text_sm()
}

/// Champ de saisie en tête d'un panneau flottant (le texte vient des touches).
pub fn search_field(field: &TextField, placeholder: &'static str) -> Div {
    div()
        .h(px(40.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(theme::border())
        .child(icon(Icon::Search, theme::text_faint()))
        .child(field.render(placeholder))
}

/// Ligne de sélection d'un panneau flottant. `id` sert aussi de groupe de survol.
pub fn list_row(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(id.clone())
        .group(id)
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .pl_2()
        .pr(px(14.0))
        .rounded_md()
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|s| s.bg(theme::hover())))
}

/// Pied d'un panneau flottant : aide, raccourcis.
pub fn popover_footer() -> Div {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_3()
        .gap_y_1()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(theme::border())
        .bg(theme::panel())
        .text_xs()
        .text_color(theme::text_faint())
}

/// « Entrée  ouvrir » : une touche suivie de son effet.
pub fn hint(keys: &'static str, action: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap_1p5().child(kbd(keys)).child(action.into())
}
