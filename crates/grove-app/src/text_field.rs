//! Champ de texte d'une ligne piloté au clavier (recherche de branche, nom
//! d'onglet) : saisie, effacement, Ctrl+A, copier / couper / coller.
//!
//! Pas de curseur déplaçable : on écrit en fin de texte. La seule sélection
//! possible est « tout » (Ctrl+A) ; sans sélection, copier et couper portent
//! sur tout le texte, comme VS Code le fait pour la ligne courante.

use gpui::{App, ClipboardItem, Div, Keystroke, div, prelude::*};

use crate::theme;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    text: String,
    /// Tout le texte est sélectionné : la prochaine saisie le remplace.
    all_selected: bool,
}

/// Effet d'une touche sur le champ.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// Touche sans rapport avec le champ : à laisser au reste de l'interface.
    Ignored,
    /// Le texte a changé.
    Changed,
    /// Seule la sélection a changé.
    Selection,
    /// Texte à placer dans le presse-papiers ; `Changed` en plus s'il a été coupé.
    Copy { text: String, cut: bool },
}

impl TextField {
    /// Champ prérempli, entièrement sélectionné (renommage).
    pub fn selected(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { all_selected: !text.is_empty(), text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Applique `keystroke`. `paste` lit le presse-papiers, seulement si besoin.
    pub fn apply(&mut self, keystroke: &Keystroke, paste: impl FnOnce() -> Option<String>) -> Edit {
        let modifiers = &keystroke.modifiers;
        let secondary = modifiers.control || modifiers.platform;
        match keystroke.key.as_str() {
            "a" if secondary => {
                self.all_selected = !self.text.is_empty();
                Edit::Selection
            }
            "c" if secondary => Edit::Copy { text: self.text.clone(), cut: false },
            "x" if secondary => {
                let text = std::mem::take(&mut self.text);
                self.all_selected = false;
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
            "backspace" if self.all_selected => {
                self.text.clear();
                self.all_selected = false;
                Edit::Changed
            }
            "backspace" if secondary => {
                self.delete_word();
                Edit::Changed
            }
            "backspace" => {
                self.text.pop();
                Edit::Changed
            }
            "left" | "right" | "home" | "end" if self.all_selected => {
                self.all_selected = false;
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

    fn insert(&mut self, text: &str) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
        }
        self.text.push_str(text);
    }

    /// Ctrl+Retour arrière : efface jusqu'au séparateur précédent (`/`, `-`, `_`, espace).
    fn delete_word(&mut self) {
        let is_separator = |c: char| matches!(c, '/' | '-' | '_' | '.' | ' ');
        let trimmed = self.text.trim_end_matches(is_separator).len();
        let start = self.text[..trimmed].rfind(is_separator).map_or(0, |i| i + 1);
        self.text.truncate(start);
    }

    /// Texte du champ, ou `placeholder` s'il est vide, suivi du curseur.
    pub fn render(&self, placeholder: &'static str) -> Div {
        if self.is_empty() {
            return div().flex().text_color(theme::text_faint()).child(placeholder);
        }
        div()
            .flex()
            .min_w_0()
            .text_color(theme::text())
            .child(
                div()
                    .when(self.all_selected, |d| d.rounded_sm().bg(theme::accent().opacity(0.28)))
                    .child(self.text.clone()),
            )
            .child(div().text_color(theme::accent()).child("▏"))
    }
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
    fn other_shortcuts_are_left_alone() {
        let mut field = typed("a");
        assert_eq!(field.apply(&key("ctrl-shift-g"), || None), Edit::Ignored);
        assert_eq!(field.apply(&key("up"), || None), Edit::Ignored);
        assert_eq!(field.as_str(), "a");
    }
}
