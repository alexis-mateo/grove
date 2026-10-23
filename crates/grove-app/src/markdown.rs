//! Markdown réduit pour les réponses de Claude : blocs (titres, listes, cases à
//! cocher, code, citations, tableaux) et, dans le texte, gras, italique, barré,
//! code et liens. Le reste s'affiche tel quel.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(String),
    /// Niveau (1 à 6) et texte.
    Heading(usize, String),
    /// Puce, numéro ou case (`•`, `2.`, `☐`, `☑`), niveau d'indentation, texte.
    ListItem { marker: String, depth: usize, text: String },
    /// Lignes consécutives commençant par `>`.
    Quote(String),
    Code(String),
    Table(Table),
    Rule,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<String>,
    pub align: Vec<Align>,
    /// Lignes complétées ou tronquées au nombre de colonnes de l'en-tête.
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Mise en forme d'un passage du texte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Bold,
    Italic,
    Strike,
    Code,
    /// Lien `[texte](url)` ou URL nue ; seuls `http(s)://` sont ouverts.
    Link(String),
}

pub fn blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();
    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush(&mut paragraph, &mut blocks);
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                code.push(line);
            }
            blocks.push(Block::Code(code.join("\n")));
        } else if trimmed.starts_with('|') {
            flush(&mut paragraph, &mut blocks);
            let mut table = vec![line];
            while let Some(next) = lines.next_if(|l| l.trim_start().starts_with('|')) {
                table.push(next);
            }
            // Sans ligne de séparation (`|---|`), ce n'est pas un tableau : chasse fixe.
            blocks.push(parse_table(&table).map_or_else(|| Block::Code(table.join("\n")), Block::Table));
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if let Some((level, title)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, title.to_owned()));
        } else if matches!(trimmed, "---" | "***" | "___") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if let Some((marker, text)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let depth = (line.len() - trimmed.len()) / 2;
            blocks.push(Block::ListItem { marker, depth, text: text.to_owned() });
        } else if let Some(quote) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            let mut quote = vec![quote.trim_start()];
            while let Some(next) = lines.next_if(|l| l.trim_start().starts_with('>')) {
                quote.push(next.trim_start()[1..].trim_start());
            }
            blocks.push(Block::Quote(quote.join("\n")));
        } else {
            paragraph.push(line);
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let title = line.trim_start_matches('#');
    let level = line.len() - title.len();
    ((1..=6).contains(&level) && title.starts_with(' ')).then(|| (level, title.trim().trim_end_matches('#').trim_end()))
}

fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(bullet) {
            for (box_, marker) in [("[ ] ", "☐"), ("[x] ", "☑"), ("[X] ", "☑")] {
                if let Some(task) = text.strip_prefix(box_) {
                    return Some((marker.into(), task));
                }
            }
            return Some(("•".into(), text));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = &line[digits..];
    if digits > 0 && (rest.starts_with(". ") || rest.starts_with(") ")) {
        return Some((format!("{}.", &line[..digits]), &rest[2..]));
    }
    None
}

/// Tableau GFM : en-tête, ligne de séparation (qui donne l'alignement), lignes.
fn parse_table(lines: &[&str]) -> Option<Table> {
    let header = cells(lines.first()?);
    let separator = cells(lines.get(1)?);
    let is_separator = |c: &String| {
        let dashes = c.trim_matches(':');
        !dashes.is_empty() && dashes.chars().all(|ch| ch == '-')
    };
    if separator.len() != header.len() || !separator.iter().all(is_separator) {
        return None;
    }
    let align = separator
        .iter()
        .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        })
        .collect();
    let rows = lines[2..]
        .iter()
        .map(|line| {
            let mut row = cells(line);
            row.resize(header.len(), String::new());
            row
        })
        .collect();
    Some(Table { header, align, rows })
}

/// Cellules d'une ligne `| a | b |` ; `\|` est un `|` dans une cellule.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').filter(|l| !l.ends_with('\\')).unwrap_or(line);
    let mut cells = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => cells.last_mut().unwrap().push(chars.next().unwrap()),
            '|' => cells.push(String::new()),
            c => cells.last_mut().unwrap().push(c),
        }
    }
    cells.into_iter().map(|c| c.trim().to_owned()).collect()
}

/// Texte sans les marqueurs (`**`, `` ` ``, `[…](…)`), et passages à mettre en forme.
/// Le gras et le libellé d'un lien peuvent contenir du code.
pub fn inline(text: &str) -> (String, Vec<(Range<usize>, Inline)>) {
    let mut out = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        // Image : son texte alternatif, en lien vers elle.
        let image = rest.strip_prefix('!').filter(|r| r.starts_with('['));
        if let Some((label, url, after)) = image.and_then(link) {
            let start = out.len();
            out.push_str(&format!("🖼 {label}"));
            if is_web(url) {
                spans.push((start..out.len(), Inline::Link(url.to_owned())));
            }
            rest = after;
        } else if let Some((label, url, after)) = link(rest) {
            let start = out.len();
            nested(label, &mut out, &mut spans);
            if is_web(url) {
                spans.push((start..out.len(), Inline::Link(url.to_owned())));
            }
            rest = after;
        } else if let Some(code) = rest.strip_prefix('`').and_then(|after| enclosed(after, "`")) {
            let start = out.len();
            out.push_str(code.0);
            spans.push((start..out.len(), Inline::Code));
            rest = code.1;
        } else if let Some(bold) = rest.strip_prefix("**").and_then(|after| enclosed(after, "**")) {
            let start = out.len();
            nested(bold.0, &mut out, &mut spans);
            spans.push((start..out.len(), Inline::Bold));
            rest = bold.1;
        } else if let Some(strike) = rest.strip_prefix("~~").and_then(|after| enclosed(after, "~~")) {
            let start = out.len();
            nested(strike.0, &mut out, &mut spans);
            spans.push((start..out.len(), Inline::Strike));
            rest = strike.1;
        } else if let Some((inner, after)) = italic(rest, &out) {
            let start = out.len();
            nested(inner, &mut out, &mut spans);
            spans.push((start..out.len(), Inline::Italic));
            rest = after;
        } else if let Some(url) = bare_url(rest).filter(|_| !out.ends_with(|c: char| c.is_alphanumeric())) {
            let start = out.len();
            out.push_str(url);
            spans.push((start..out.len(), Inline::Link(url.to_owned())));
            rest = &rest[url.len()..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    (out, spans)
}

/// Ajoute `text` mis en forme à `out`, en décalant ses passages.
fn nested(text: &str, out: &mut String, spans: &mut Vec<(Range<usize>, Inline)>) {
    let (inner, inner_spans) = inline(text);
    let offset = out.len();
    out.push_str(&inner);
    spans.extend(inner_spans.into_iter().map(|(range, style)| (range.start + offset..range.end + offset, style)));
}

/// `*x*` ou `_x_` au début de `rest` : collé au texte qu'il entoure (`a * b`
/// n'en est pas) et hors d'un mot (`snake_case`, `2*3*4` restent tels quels).
fn italic<'a>(rest: &'a str, before: &str) -> Option<(&'a str, &'a str)> {
    let marker = rest.chars().next().filter(|c| matches!(c, '*' | '_'))?;
    if before.ends_with(|c: char| c.is_alphanumeric()) {
        return None;
    }
    let after = &rest[1..];
    if after.starts_with([marker, ' ']) {
        return None;
    }
    let end = after.find(marker).filter(|&end| end > 0)?;
    let (inner, tail) = (&after[..end], &after[end + 1..]);
    let closes = !inner.ends_with(' ') && !inner.contains('\n');
    let bounded = !tail.starts_with(|c: char| c.is_alphanumeric());
    (closes && bounded).then_some((inner, tail))
}

/// Contenu non vide jusqu'au prochain `marker`, et ce qui le suit.
fn enclosed<'a>(text: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let end = text.find(marker).filter(|&end| end > 0)?;
    Some((&text[..end], &text[end + marker.len()..]))
}

/// `[libellé](cible)` au début de `text` : libellé, cible, suite.
fn link(text: &str) -> Option<(&str, &str, &str)> {
    let after = text.strip_prefix('[')?;
    let close = after.find("](")?;
    let label = &after[..close];
    if label.is_empty() || label.contains(['[', '\n']) {
        return None;
    }
    let target = &after[close + 2..];
    let end = target.find(')')?;
    let url = &target[..end];
    (!url.is_empty() && !url.contains(char::is_whitespace)).then(|| (label, url, &target[end + 1..]))
}

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// URL nue au début de `text`, sans la ponctuation qui la suit.
fn bare_url(text: &str) -> Option<&str> {
    if !is_web(text) {
        return None;
    }
    let end = text.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).unwrap_or(text.len());
    let url = text[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'']);
    (url.len() > "https://".len()).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_blocks() {
        let text = "# Titre\n\nUn paragraphe\nsur deux lignes.\n\n- un\n  - deux\n3. trois\n\n```rust\nfn main() {}\n```\n| a | b |\n> cité\n---";
        assert_eq!(blocks(text), vec![
            Block::Heading(1, "Titre".into()),
            Block::Paragraph("Un paragraphe\nsur deux lignes.".into()),
            Block::ListItem { marker: "•".into(), depth: 0, text: "un".into() },
            Block::ListItem { marker: "•".into(), depth: 1, text: "deux".into() },
            Block::ListItem { marker: "3.".into(), depth: 0, text: "trois".into() },
            Block::Code("fn main() {}".into()),
            Block::Code("| a | b |".into()),
            Block::Quote("cité".into()),
            Block::Rule,
        ]);
    }

    #[test]
    fn inline_bold_and_code() {
        let (text, spans) = inline("Lance **vite** `cargo test` puis * et ` seuls");
        assert_eq!(text, "Lance vite cargo test puis * et ` seuls");
        assert_eq!(spans, vec![(6..10, Inline::Bold), (11..21, Inline::Code)]);
        assert_eq!(inline("é**à**"), ("éà".into(), vec![(2..4, Inline::Bold)]));
    }

    #[test]
    fn tables() {
        let text = "| Fichier | Lignes | État |\n|:--|--:|:-:|\n| `a.rs` | 12 | ok |\n| b\\|c | 3 |";
        assert_eq!(blocks(text), vec![Block::Table(Table {
            header: vec!["Fichier".into(), "Lignes".into(), "État".into()],
            align: vec![Align::Left, Align::Right, Align::Center],
            rows: vec![
                vec!["`a.rs`".into(), "12".into(), "ok".into()],
                vec!["b|c".into(), "3".into(), String::new()],
            ],
        })]);
    }

    #[test]
    fn quotes_tasks_and_headings() {
        assert_eq!(blocks("> un\n> deux\n\n## Titre ##\n- [ ] à faire\n- [x] fait"), vec![
            Block::Quote("un\ndeux".into()),
            Block::Heading(2, "Titre".into()),
            Block::ListItem { marker: "☐".into(), depth: 0, text: "à faire".into() },
            Block::ListItem { marker: "☑".into(), depth: 0, text: "fait".into() },
        ]);
    }

    #[test]
    fn italic_strike_and_images() {
        assert_eq!(inline("un *mot* et _deux_"), ("un mot et deux".into(), vec![(3..6, Inline::Italic), (10..14, Inline::Italic)]));
        assert_eq!(inline("a * b * c, snake_case_name, 2*3*4"), ("a * b * c, snake_case_name, 2*3*4".into(), vec![]));
        assert_eq!(inline("~~vieux~~"), ("vieux".into(), vec![(0..5, Inline::Strike)]));
        let (text, spans) = inline("![schéma](https://x.io/a.png)");
        assert_eq!(text, "🖼 schéma");
        assert_eq!(spans, vec![(0..text.len(), Inline::Link("https://x.io/a.png".into()))]);
    }

    #[test]
    fn links_and_nesting() {
        let (text, spans) = inline("PR [#9016](https://github.com/o/r/pull/9016) à jour.");
        assert_eq!(text, "PR #9016 à jour.");
        assert_eq!(spans, vec![(3..8, Inline::Link("https://github.com/o/r/pull/9016".into()))]);

        let (text, spans) = inline("voir https://example.com/a_b. Fin");
        assert_eq!(text, "voir https://example.com/a_b. Fin");
        assert_eq!(spans, vec![(5..28, Inline::Link("https://example.com/a_b".into()))]);

        // Cible locale : le libellé seul, sans lien.
        assert_eq!(inline("[main.rs](src/main.rs)"), ("main.rs".into(), vec![]));
        // Code dans le gras et dans un libellé.
        let (text, spans) = inline("**Ordre et `createdAt`**");
        assert_eq!(text, "Ordre et createdAt");
        assert_eq!(spans, vec![(9..18, Inline::Code), (0..18, Inline::Bold)]);
        // Pas de lien dans du code.
        assert_eq!(inline("`https://x.io`"), ("https://x.io".into(), vec![(0..12, Inline::Code)]));
    }

    #[test]
    fn hashes_without_space_are_text() {
        assert_eq!(blocks("#123 corrigé"), vec![Block::Paragraph("#123 corrigé".into())]);
    }
}
