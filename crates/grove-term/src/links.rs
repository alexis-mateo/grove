//! Liens cliquables : hyperliens OSC 8, URL et chemins de fichiers détectés dans le texte.

use std::path::{Path, PathBuf};

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

/// Caractères d'un chemin : ni blanc, ni guillemet, ni parenthèse (`Read(src/a.rs)`),
/// ni trait de cadre (le TUI de Claude Code en dessine autour du texte).
const PATH_CHAR: &str = r#"[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"'\s{}()\^⟨⟩`|\u{2500}-\u{257F}]"#;

/// Lien sous une cellule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// URL, ou texte du chemin tel qu'affiché.
    pub uri: String,
    /// Fichier ou dossier existant, à ouvrir (éditeur configuré, sinon application par défaut).
    pub path: Option<PathBuf>,
    /// Suffixe `:ligne` ou `:ligne:colonne` d'un chemin.
    pub line: Option<u32>,
    pub column: Option<u32>,
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

/// Expressions compilées une fois (la construction des automates coûte plus que
/// la recherche), et dossier contre lequel résoudre les chemins relatifs.
pub(crate) struct LinkFinder {
    url: RegexSearch,
    path: RegexSearch,
    base: Option<PathBuf>,
}

impl LinkFinder {
    pub fn new(base: Option<PathBuf>) -> Self {
        Self {
            url: RegexSearch::new(URL_REGEX).expect("expression des URL valide"),
            // Un mot qui contient au moins un `/` ; son existence est vérifiée ensuite.
            path: RegexSearch::new(&format!("{PATH_CHAR}*/{PATH_CHAR}+")).expect("expression des chemins valide"),
            base,
        }
    }

    /// Lien qui couvre `point` (coordonnées de la grille).
    pub fn link_at<T>(&mut self, term: &Term<T>, point: Point) -> Option<Link> {
        link_at(term, &mut self.url, &mut self.path, self.base.as_deref(), point)
    }
}

fn link_at<T>(
    term: &Term<T>,
    url_regex: &mut RegexSearch,
    path_regex: &mut RegexSearch,
    base: Option<&Path>,
    point: Point,
) -> Option<Link> {
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
        return Some(Link { uri: hyperlink.uri().to_owned(), path: None, line: None, column: None, start: visible(start), end: visible(end) });
    }

    let line_start = term.line_search_left(point);
    let line_end = term.line_search_right(point);
    let matched = |regex: &mut RegexSearch| {
        let found = RegexIter::new(line_start, line_end, Direction::Right, term, regex).find(|m| m.contains(&point))?;
        let text = term.bounds_to_string(*found.start(), *found.end());
        let kept = trim_url(&text).to_owned();
        // Les caractères retirés en fin de texte sont tous ASCII : une cellule chacun.
        let end = found.end().sub(term, Boundary::Grid, text.chars().count() - kept.chars().count());
        (point <= end).then(|| (kept, visible(*found.start()), visible(end)))
    };

    if let Some((uri, start, end)) = matched(url_regex) {
        return Some(Link { uri, path: None, line: None, column: None, start, end });
    }
    let (text, start, end) = matched(path_regex)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (path, line, column) = resolve_path(&text, base, home.as_deref())?;
    Some(Link { uri: text, path: Some(path), line, column, start, end })
}

/// Fichier ou dossier désigné par `text`, s'il existe : absolu, sous `~/`, ou
/// relatif à `base`, avec la ligne et la colonne d'un suffixe `:ligne` ou `:ligne:colonne`.
fn resolve_path(text: &str, base: Option<&Path>, home: Option<&Path>) -> Option<(PathBuf, Option<u32>, Option<u32>)> {
    if text.contains("://") {
        return None;
    }
    let mut text = text;
    // Lus de la fin : la colonne, puis la ligne.
    let mut numbers = Vec::new();
    for _ in 0..2 {
        if let Some((rest, number)) = text.rsplit_once(':')
            && let Ok(number) = number.parse::<u32>()
        {
            text = rest;
            numbers.push(number);
        }
    }
    let (line, column) = match numbers[..] {
        [column, line] => (Some(line), Some(column)),
        [line] => (Some(line), None),
        _ => (None, None),
    };
    let path = if let Some(rest) = text.strip_prefix("~/") {
        home?.join(rest)
    } else if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        base?.join(text)
    };
    path.exists().then_some((path, line, column))
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
    fn resolves_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes-2026.txt");
        std::fs::write(&file, "").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let base = Some(dir.path());
        let main = dir.path().join("src/main.rs");
        let path = |text: &str, base, home| resolve_path(text, base, home).map(|(path, ..)| path);

        assert_eq!(path(file.to_str().unwrap(), None, None), Some(file.clone()));
        assert_eq!(resolve_path("src/main.rs", base, None), Some((main.clone(), None, None)));
        assert_eq!(resolve_path("src/main.rs:42", base, None), Some((main.clone(), Some(42), None)));
        assert_eq!(resolve_path("src/main.rs:42:7", base, None), Some((main, Some(42), Some(7))));
        assert_eq!(path("./src", base, None), Some(dir.path().join("./src")));
        assert_eq!(path("~/notes-2026.txt", None, base), Some(dir.path().join("notes-2026.txt")));
        // Inexistant, ou relatif sans dossier de base : pas un lien.
        assert_eq!(resolve_path("src/absent.rs", base, None), None);
        assert_eq!(resolve_path("et/ou", base, None), None);
        assert_eq!(resolve_path("src/main.rs", None, None), None);
        assert_eq!(resolve_path("javascript://x", base, None), None);
    }

    #[test]
    fn only_known_schemes_are_openable() {
        assert!(is_openable("https://github.com"));
        assert!(is_openable("FILE:///tmp/x"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("x-scheme-handler:foo"));
    }
}
