//! Palette de commandes (`Ctrl+Shift+K`) : les actions de Grove avec leur
//! raccourci, et les espaces de travail, filtrés au fil de la frappe.

use gpui::{Action, Context, FocusHandle, KeyDownEvent, SharedString, Window, div, prelude::*, px, uniform_list};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::list::{ListScroll, ROW_HEIGHT};
use crate::marquee::marquee;
use crate::text_field::{self, Edit, TextField};
use crate::theme;
use crate::ui::{Icon, hint, icon, kbd, list_row, popover, popover_footer, search_field};

pub(crate) struct Palette {
    query: TextField,
    selected: usize,
    scroll: ListScroll,
    pub focus: FocusHandle,
    items: Vec<Item>,
}

struct Item {
    label: String,
    /// Groupe de l'action, ou dossier de l'espace.
    detail: String,
    keys: String,
    target: Target,
    /// Texte cherché : libellé, détail et touches, sans casse ni accents.
    haystack: String,
}

enum Target {
    Action(Box<dyn Action>),
    Workspace(WorkspaceKey),
}

impl Palette {
    /// Index des éléments retenus par la recherche : chaque mot doit apparaître.
    fn matches(&self) -> Vec<usize> {
        let query = fold(self.query.as_str());
        let words: Vec<&str> = query.split_whitespace().collect();
        (0..self.items.len()).filter(|&i| words.iter().all(|w| self.items[i].haystack.contains(w))).collect()
    }
}

/// Minuscules sans accents : « Écart » trouve « ecart ».
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '+' | '-' => ' ',
            c => c,
        })
        .collect()
}

fn item(label: String, detail: String, keys: String, target: Target) -> Item {
    let haystack = fold(&format!("{label} {detail} {keys}"));
    Item { label, detail, keys, target, haystack }
}

impl GroveView {
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.branch_picker = None;
        self.profile_picker = None;
        let mut items: Vec<Item> = crate::shortcuts::commands(&crate::grove_view::key_bindings())
            .into_iter()
            .filter(|(action, _)| action.name() != "grove::CommandPalette")
            .map(|(action, s)| item(s.description.to_owned(), s.group.label().to_owned(), s.keys, Target::Action(action)))
            .collect();
        for workspace in self.state.projects.iter().flat_map(|p| p.workspaces.iter().map(move |w| (p, w))) {
            let (project, workspace) = workspace;
            let branch = self.git.get(&workspace.id).and_then(|g| g.head.as_ref()).map(|h| h.label().to_owned());
            let name = match (&workspace.kind, branch) {
                (grove_core::WorkspaceKind::Worktree { branch, .. }, _) => branch.clone(),
                (grove_core::WorkspaceKind::Direct, Some(branch)) => format!("{branch} (dossier principal)"),
                (grove_core::WorkspaceKind::Direct, None) => "dossier principal".to_owned(),
            };
            let label = format!("Aller à {} › {name}", project.name);
            let detail = workspace.path.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~");
            items.push(item(label, detail, String::new(), Target::Workspace((project.id, workspace.id))));
        }
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.palette = Some(Palette { query: TextField::default(), selected: 0, scroll: ListScroll::default(), focus, items });
        cx.notify();
    }

    fn run_palette_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.take() else { return };
        let Some(item) = palette.items.into_iter().nth(index) else { return };
        // L'action part de l'onglet actif, comme si son raccourci y avait été tapé.
        self.focus_active(window, cx);
        match item.target {
            Target::Action(action) => window.dispatch_action(action, cx),
            Target::Workspace(key) => self.activate_workspace(key, window, cx),
        }
        cx.notify();
    }

    pub(crate) fn palette_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = &mut self.palette else { return };
        let keystroke = &event.keystroke;
        let matches = palette.matches();
        let count = matches.len();
        match keystroke.key.as_str() {
            "escape" => return self.toggle_palette(window, cx),
            "enter" => {
                if let Some(&index) = matches.get(palette.selected) {
                    return self.run_palette_item(index, window, cx);
                }
            }
            "up" => {
                palette.selected = palette.selected.saturating_sub(1);
                palette.scroll.reveal(palette.selected, count);
            }
            "down" => {
                palette.selected = (palette.selected + 1).min(count.saturating_sub(1));
                palette.scroll.reveal(palette.selected, count);
            }
            _ => match palette.query.key_down(keystroke, cx) {
                Edit::Ignored => return,
                Edit::Changed | Edit::Copy { cut: true, .. } => {
                    palette.selected = 0;
                    palette.scroll.reset();
                }
                Edit::Selection | Edit::Copy { .. } => {}
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn palette_row(&self, row: usize, index: usize, selected: bool, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let item = self.palette.as_ref()?.items.get(index)?;
        let glyph = match item.target {
            Target::Action(_) => Icon::ChevronRight,
            Target::Workspace(_) => Icon::Folder,
        };
        Some(
            list_row(format!("palette-item-{row}"), selected)
                .h(ROW_HEIGHT)
                .on_click(cx.listener(move |this, _, window, cx| this.run_palette_item(index, window, cx)))
                .child(icon(glyph, theme::text_faint()))
                .child(div().flex().flex_shrink_0().max_w(px(330.0)).text_color(theme::text()).child(marquee(
                    SharedString::from(format!("palette-label-{row}")),
                    item.label.clone(),
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child(item.detail.clone()),
                )
                .when(!item.keys.is_empty(), |d| d.child(kbd(item.keys.clone())))
                .into_any_element(),
        )
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let palette = self.palette.as_ref()?;
        let count = palette.matches().len();
        let list = div()
            .relative()
            .h(ListScroll::height(count))
            .flex_shrink()
            .child(
                uniform_list(
                    "palette-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let Some(palette) = &this.palette else { return Vec::new() };
                        let (matches, selected) = (palette.matches(), palette.selected);
                        range.filter_map(|row| this.palette_row(row, *matches.get(row)?, row == selected, cx)).collect()
                    }),
                )
                .track_scroll(palette.scroll.handle.clone())
                .size_full(),
            )
            .children(palette.scroll.scrollbar(count));
        let panel = popover("command-palette", &palette.focus, px(640.0))
            // Centrée en haut, par-dessus les onglets.
            .relative()
            .top_0()
            .left_0()
            .on_key_down(cx.listener(Self::palette_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.popover_dismissed = true;
                this.toggle_palette(window, cx);
            }))
            .child(search_field(&palette.query, "Commande ou espace de travail…"))
            .child(text_field::input_target(&palette.focus, cx.entity()))
            .child(div().p_1().flex().flex_col().flex_shrink().min_h_0().child(list))
            .when(count == 0, |d| d.child(div().px_3().py_2().text_color(theme::text_muted()).child("Aucune commande.")))
            .child(
                popover_footer()
                    .child(hint("↑↓", "naviguer"))
                    .child(hint("Entrée", "lancer"))
                    .child(hint("Échap", "fermer")),
            );
        Some(
            div()
                .absolute()
                .top(crate::window_frame::TOP_BAR_HEIGHT + px(6.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(palette.scroll.track_drag(panel, count))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn search_ignores_case_and_accents() {
        assert_eq!(fold("Écart Sélecteur"), "ecart selecteur");
        assert_eq!(fold("Ctrl+Shift+E"), "ctrl shift e");
    }
}
