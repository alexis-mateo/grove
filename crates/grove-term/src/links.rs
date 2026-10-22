//! Liens cliquables : hyperliens OSC 8 et URL détectées dans le texte.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Direction, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

/// Schémas reconnus dans le texte, et seuls ouverts depuis un hyperlien OSC 8 :
/// un programme ne doit pas pouvoir faire lancer n'importe quel gestionnaire d'URL.
const SCHEMES: &[&str] = &["https://", "http://", "file://", "mailto:", "ftp://", "ssh://", "git://"];

/// Comme dans Alacritty : un schéma puis tout jusqu'à un blanc, un caractère de
/// contrôle ou un délimiteur courant.
const URL_REGEX: &str = r#"(https://|http://|file://|mailto:|ftp://|ssh://|git://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{}\^⟨⟩`]+"#;

/// Lien sous une cellule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub uri: String,
    /// Première et dernière cellule (ligne visible, colonne), bornes incluses.
    /// Un lien peut s'étendre sur plusieurs lignes (retour à la ligne automatique).
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Link {
    pub fn contains(&self, row: usize, column: usize) -> bool {
        (self.start..=self.end).contains(&(row, column))
    }
}

/// L'URI peut être ouverte sans risque (schéma connu).
pub fn is_openable(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    SCHEMES.iter().any(|scheme| lower.starts_with(scheme))
}

pub(crate) fn url_regex() -> RegexSearch {
    RegexSearch::new(URL_REGEX).expect("expression des URL valide")
}

/// Lien qui couvre `point` (coordonnées de la grille).
pub(crate) fn link_at<T>(term: &Term<T>, regex: &mut RegexSearch, point: Point) -> Option<Link> {
    let offset = term.grid().display_offset() as i32;
    let lines = term.screen_lines() as i32;
    let last_column = term.columns().saturating_sub(1);
    // Ligne de la grille → ligne visible, bornée à l'écran.
    let visible = |p: Point| -> (usize, usize) {
        let row = p.line.0 + offset;
        if row < 0 {
            (0, 0)
        } else if row >= lines {
            (lines as usize - 1, last_column)
        } else {
            (row as usize, p.column.0)
        }
    };

    if let Some(hyperlink) = term.grid()[point].hyperlink() {
        if !is_openable(hyperlink.uri()) {
            return None;
        }
        let same = |p: Point| term.grid()[p].hyperlink().as_ref() == Some(&hyperlink);
        let (mut start, mut end) = (point, point);
        loop {
            let previous = start.sub(term, Boundary::Grid, 1);
            if previous == start || !same(previous) {
                break;
            }
            start = previous;
        }
        loop {
            let next = end.add(term, Boundary::Grid, 1);
            if next == end || !same(next) {
                break;
            }
            end = next;
        }
        return Some(Link { uri: hyperlink.uri().to_owned(), start: visible(start), end: visible(end) });
    }

    let line_start = term.line_search_left(point);
    let line_end = term.line_search_right(point);
    let found = RegexIter::new(line_start, line_end, Direction::Right, term, regex)
        .find(|m| m.contains(&point))?;
    let text = term.bounds_to_string(*found.start(), *found.end());
    let uri = trim_url(&text);
    // Les caractères retirés en fin d'URL sont tous ASCII : une cellule chacun.
    let end = found.end().sub(term, Boundary::Grid, text.chars().count() - uri.chars().count());
    if point > end {
        return None;
    }
    Some(Link { uri: uri.to_owned(), start: visible(*found.start()), end: visible(end) })
}

/// Retire la ponctuation qui suit souvent une URL dans une phrase, ainsi que
/// les parenthèses et crochets fermants sans ouvrant (`(voir https://x.y)`).
fn trim_url(url: &str) -> &str {
    let mut url = url;
    loop {
        let Some(last) = url.chars().last() else { return url };
        let unbalanced = |open: char| url.matches(open).count() < url.matches(last).count();
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => unbalanced('('),
            ']' => unbalanced('['),
            _ => false,
        };
        if !strip {
            return url;
        }
        url = &url[..url.len() - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_sentence_punctuation() {
        assert_eq!(trim_url("https://x.y/a."), "https://x.y/a");
        assert_eq!(trim_url("https://x.y/a)."), "https://x.y/a");
        assert_eq!(trim_url("https://en.wikipedia.org/wiki/Rust_(langage)"), "https://en.wikipedia.org/wiki/Rust_(langage)");
        assert_eq!(trim_url("https://x.y/?q=1"), "https://x.y/?q=1");
    }

    #[test]
    fn only_known_schemes_are_openable() {
        assert!(is_openable("https://github.com"));
        assert!(is_openable("FILE:///tmp/x"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("x-scheme-handler:foo"));
    }
}
