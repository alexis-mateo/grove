//! Champ de texte d'une ligne piloté au clavier (recherche de branche, nom
//! d'onglet, Paramètres) : curseur déplaçable, sélection, copier / couper / coller.
//!
//! Flèches, `Début`, `Fin` ; avec `Ctrl`, de mot en mot ; avec `Shift`, la
//! sélection s'étend. Sans sélection, copier et couper portent sur tout le
//! texte, comme VS Code le fait pour la ligne courante.

use std::ops::Range;

use gpui::{
    App, ClipboardItem, Div, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Keystroke, canvas, div,
    prelude::*, px,
};

use crate::theme;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    text: String,
    /// Position du curseur, en octets (toujours sur une frontière de caractère).
    cursor: usize,
    /// Autre bout de la sélection ; égal au curseur ou `None` : pas de sélection.
    anchor: Option<usize>,
}

/// Effet d'une touche sur le champ.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// Touche sans rapport avec le champ : à laisser au reste de l'interface.
    Ignored,
    /// Le texte a changé.
    Changed,
    /// Seuls le curseur ou la sélection ont changé.
    Selection,
    /// Texte à placer dans le presse-papiers ; `Changed` en plus s'il a été coupé.
    Copy { text: String, cut: bool },
}

/// Caractère d'un mot, pour Ctrl+flèches et Ctrl+Retour arrière.
fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

impl TextField {
    /// Champ prérempli, entièrement sélectionné (renommage) : la saisie le remplace.
    pub fn selected(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { cursor: text.len(), anchor: Some(0).filter(|_| !text.is_empty()), text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Partie sélectionnée, si elle n'est pas vide.
    fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    fn previous(&self, from: usize) -> usize {
        self.text[..from].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, from: usize) -> usize {
        self.text[from..].chars().next().map_or(from, |c| from + c.len_utf8())
    }

    /// Début du mot avant `from` : on saute les séparateurs, puis le mot.
    fn word_start(&self, from: usize) -> usize {
        let before = &self.text[..from];
        let trimmed = before.trim_end_matches(|c| !is_word(c));
        trimmed.trim_end_matches(is_word).len()
    }

    /// Fin du mot après `from`.
    fn word_end(&self, from: usize) -> usize {
        let after = &self.text[from..];
        let trimmed = after.trim_start_matches(|c| !is_word(c));
        let word = trimmed.len() - trimmed.trim_start_matches(is_word).len();
        from + (after.len() - trimmed.len()) + word
    }

    /// Place le curseur en `to` ; avec `extend`, la sélection part de l'ancienne position.
    fn move_to(&mut self, to: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
    }

    fn delete(&mut self, range: Range<usize>) {
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = None;
    }

    /// Remplace la sélection (ou insère au curseur).
    fn insert(&mut self, text: &str) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = None;
    }

    /// Applique `keystroke`. `paste` lit le presse-papiers, seulement si besoin.
    pub fn apply(&mut self, keystroke: &Keystroke, paste: impl FnOnce() -> Option<String>) -> Edit {
        let modifiers = &keystroke.modifiers;
        let secondary = modifiers.control || modifiers.platform;
        let shift = modifiers.shift;
        let selection = self.selection();
        match keystroke.key.as_str() {
            "a" if secondary => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
                Edit::Selection
            }
            "c" if secondary => match selection {
                Some(range) => Edit::Copy { text: self.text[range].to_owned(), cut: false },
                None => Edit::Copy { text: self.text.clone(), cut: false },
            },
            "x" if secondary => {
                let range = selection.unwrap_or(0..self.text.len());
                let text = self.text[range.clone()].to_owned();
                self.delete(range);
                Edit::Copy { text, cut: true }
            }
            "v" if secondary => match paste() {
                // Une seule ligne : les retours à la ligne du presse-papiers sont retirés.
                Some(text) => {
                    let line: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                    self.insert(line.trim());
                    Edit::Changed
                }
                None => Edit::Selection,
            },
            "backspace" | "delete" if selection.is_some() => {
                self.delete(selection.unwrap_or_default());
                Edit::Changed
            }
            "backspace" => {
                let start = if secondary { self.word_start(self.cursor) } else { self.previous(self.cursor) };
                self.delete(start..self.cursor);
                Edit::Changed
            }
            "delete" => {
                let end = if secondary { self.word_end(self.cursor) } else { self.next(self.cursor) };
                self.delete(self.cursor..end);
                Edit::Changed
            }
            // Sans Shift, une flèche réduit d'abord la sélection à son bord.
            "left" if !shift && selection.is_some() => {
                self.move_to(selection.unwrap_or_default().start, false);
                Edit::Selection
            }
            "right" if !shift && selection.is_some() => {
                self.move_to(selection.unwrap_or_default().end, false);
                Edit::Selection
            }
            "left" => {
                let to = if secondary { self.word_start(self.cursor) } else { self.previous(self.cursor) };
                self.move_to(to, shift);
                Edit::Selection
            }
            "right" => {
                let to = if secondary { self.word_end(self.cursor) } else { self.next(self.cursor) };
                self.move_to(to, shift);
                Edit::Selection
            }
            "home" => {
                self.move_to(0, shift);
                Edit::Selection
            }
            "end" => {
                self.move_to(self.text.len(), shift);
                Edit::Selection
            }
            _ if secondary => Edit::Ignored,
            _ => match &keystroke.key_char {
                Some(text) if !text.chars().any(char::is_control) => {
                    self.insert(text);
                    Edit::Changed
                }
                _ => Edit::Ignored,
            },
        }
    }

    /// Variante de `apply` qui lit et écrit le presse-papiers de l'application.
    pub fn key_down(&mut self, keystroke: &Keystroke, cx: &mut App) -> Edit {
        let edit = self.apply(keystroke, || cx.read_from_clipboard().and_then(|item| item.text()));
        if let Edit::Copy { text, .. } = &edit
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        edit
    }

    /// Texte validé par une méthode de saisie (lettres accentuées, touches mortes) :
    /// la touche équivalente, sans nom pour qu'aucun raccourci ne s'y applique.
    pub fn composed(text: &str) -> Keystroke {
        Keystroke { modifiers: Default::default(), key: String::new(), key_char: Some(text.to_owned()) }
    }

    /// Texte du champ avec le curseur et la sélection, ou `placeholder` s'il est vide.
    pub fn render(&self, placeholder: &'static str) -> Div {
        let caret = || div().flex_none().w(px(1.5)).h(px(15.0)).bg(theme::accent());
        if self.is_empty() {
            return div().flex().items_center().child(caret()).child(div().text_color(theme::text_faint()).child(placeholder));
        }
        let span = |text: &str| div().flex_none().whitespace_nowrap().child(text.to_owned());
        let (start, end) = self.selection().map_or((self.cursor, self.cursor), |r| (r.start, r.end));
        let selected = span(&self.text[start..end]).rounded_sm().bg(theme::accent().opacity(0.28));
        let (before, middle, after) = (&self.text[..start], selected, &self.text[end..]);
        let row = div().flex().items_center().min_w_0().text_color(theme::text()).child(span(before));
        // Le curseur est au bord de la sélection où il se trouve.
        if self.cursor == start {
            row.child(caret()).when(start != end, |d| d.child(middle)).child(span(after))
        } else {
            row.child(middle).child(caret()).child(span(after))
        }
    }
}

/// À placer dans l'élément qui porte `focus` : tant qu'il a le focus, le texte
/// composé par la méthode de saisie (é, è, ê, ç…, qui n'arrivent pas tous en
/// frappes) est remis à `view`.
pub fn input_target<V: EntityInputHandler>(focus: &FocusHandle, view: Entity<V>) -> impl IntoElement {
    let focus = focus.clone();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx),
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &str) -> Keystroke {
        let mut keystroke = Keystroke::parse(spec).unwrap();
        if keystroke.key.chars().count() == 1 && !keystroke.modifiers.control {
            keystroke.key_char = Some(keystroke.key.clone());
        }
        keystroke
    }

    fn typed(text: &str) -> TextField {
        let mut field = TextField::default();
        for c in text.chars() {
            field.apply(&key(&c.to_string()), || None);
        }
        field
    }

    #[test]
    fn select_all_then_type_replaces() {
        let mut field = typed("feat");
        assert_eq!(field.apply(&key("ctrl-a"), || None), Edit::Selection);
        field.apply(&key("x"), || None);
        assert_eq!(field.as_str(), "x");
    }

    #[test]
    fn copy_cut_paste() {
        let mut field = typed("fix");
        assert_eq!(field.apply(&key("ctrl-c"), || None), Edit::Copy { text: "fix".into(), cut: false });
        assert_eq!(field.as_str(), "fix");
        assert_eq!(field.apply(&key("ctrl-x"), || None), Edit::Copy { text: "fix".into(), cut: true });
        assert!(field.is_empty());
        field.apply(&key("ctrl-v"), || Some(" feat-12\n".into()));
        field.apply(&key("ctrl-v"), || Some("/ui".into()));
        assert_eq!(field.as_str(), "feat-12/ui");
        // Coller sur une sélection la remplace.
        field.apply(&key("ctrl-a"), || None);
        field.apply(&key("ctrl-v"), || Some("main".into()));
        assert_eq!(field.as_str(), "main");
    }

    #[test]
    fn backspace_variants() {
        let mut field = typed("feat/ui-redesign");
        field.apply(&key("ctrl-backspace"), || None);
        assert_eq!(field.as_str(), "feat/ui-");
        field.apply(&key("ctrl-backspace"), || None);
        assert_eq!(field.as_str(), "feat/");
        field.apply(&key("backspace"), || None);
        assert_eq!(field.as_str(), "feat");
        field.apply(&key("ctrl-a"), || None);
        field.apply(&key("backspace"), || None);
        assert!(field.is_empty());
    }

    #[test]
    fn composed_text_is_inserted() {
        let mut field = typed("caf");
        assert_eq!(field.apply(&TextField::composed("é"), || None), Edit::Changed);
        assert_eq!(field.as_str(), "café");
    }

    #[test]
    fn cursor_moves_and_edits_in_the_middle() {
        let mut field = typed("feat/login");
        for _ in 0..5 {
            field.apply(&key("left"), || None);
        }
        field.apply(&key("x"), || None);
        assert_eq!(field.as_str(), "feat/xlogin");
        field.apply(&key("backspace"), || None);
        field.apply(&key("delete"), || None);
        assert_eq!(field.as_str(), "feat/ogin");
        field.apply(&key("home"), || None);
        field.apply(&key("é"), || None);
        assert_eq!(field.as_str(), "éfeat/ogin");
        field.apply(&key("end"), || None);
        field.apply(&key("s"), || None);
        assert_eq!(field.as_str(), "éfeat/ogins");
    }

    #[test]
    fn word_jumps_and_shift_selection() {
        let mut field = typed("fix ui-redesign");
        field.apply(&key("ctrl-left"), || None);
        assert_eq!(field.cursor, "fix ui-".len());
        field.apply(&key("ctrl-left"), || None);
        assert_eq!(field.cursor, "fix ".len());
        field.apply(&key("ctrl-right"), || None);
        field.apply(&key("right"), || None);
        field.apply(&key("ctrl-shift-right"), || None);
        assert_eq!(field.apply(&key("ctrl-c"), || None), Edit::Copy { text: "redesign".into(), cut: false });
        // Sans Shift, la flèche réduit la sélection à son bord.
        field.apply(&key("left"), || None);
        assert_eq!(field.cursor, "fix ui-".len());
        field.apply(&key("shift-home"), || None);
        field.apply(&key("y"), || None);
        assert_eq!(field.as_str(), "yredesign");
        field.apply(&key("ctrl-delete"), || None);
        assert_eq!(field.as_str(), "y");
    }

    #[test]
    fn multibyte_characters_are_never_split() {
        let mut field = typed("çà");
        field.apply(&key("left"), || None);
        field.apply(&key("backspace"), || None);
        assert_eq!(field.as_str(), "à");
        field.apply(&key("right"), || None);
        field.apply(&key("right"), || None);
        assert_eq!(field.cursor, "à".len());
    }

    #[test]
    fn other_shortcuts_are_left_alone() {
        let mut field = typed("a");
        assert_eq!(field.apply(&key("ctrl-shift-g"), || None), Edit::Ignored);
        assert_eq!(field.apply(&key("up"), || None), Edit::Ignored);
        assert_eq!(field.as_str(), "a");
    }
}
