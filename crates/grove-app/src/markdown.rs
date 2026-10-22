//! Markdown réduit pour les réponses de Claude : blocs (titres, listes, code,
//! citations) et, dans le texte, gras et code. Le reste s'affiche tel quel.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(String),
    Heading(String),
    /// Puce ou numéro (`•`, `2.`), niveau d'indentation, texte.
    ListItem { marker: String, depth: usize, text: String },
    Quote(String),
    /// Bloc de code ; les tableaux y sont rangés aussi, la police à chasse fixe garde leurs colonnes.
    Code(String),
    Rule,
}

/// Mise en forme d'un passage du texte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    Bold,
    Code,
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
            blocks.push(Block::Code(table.join("\n")));
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if let Some(title) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(title.to_owned()));
        } else if matches!(trimmed, "---" | "***" | "___") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if let Some((marker, text)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let depth = (line.len() - trimmed.len()) / 2;
            blocks.push(Block::ListItem { marker, depth, text: text.to_owned() });
        } else if let Some(quote) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(quote.trim_start().to_owned()));
        } else {
            paragraph.push(line);
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn heading(line: &str) -> Option<&str> {
    let title = line.trim_start_matches('#');
    let level = line.len() - title.len();
    ((1..=6).contains(&level) && title.starts_with(' ')).then(|| title.trim())
}

fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(bullet) {
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

/// Texte sans les marqueurs `**` et `` ` ``, et passages à mettre en forme.
pub fn inline(text: &str) -> (String, Vec<(Range<usize>, Inline)>) {
    let mut out = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let marked = [("`", Inline::Code), ("**", Inline::Bold)].into_iter().find_map(|(marker, style)| {
            let after = rest.strip_prefix(marker)?;
            let end = after.find(marker).filter(|&end| end > 0)?;
            Some((style, &after[..end], &after[end + marker.len()..]))
        });
        match marked {
            Some((style, inner, after)) => {
                let start = out.len();
                out.push_str(inner);
                spans.push((start..out.len(), style));
                rest = after;
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_blocks() {
        let text = "# Titre\n\nUn paragraphe\nsur deux lignes.\n\n- un\n  - deux\n3. trois\n\n```rust\nfn main() {}\n```\n| a | b |\n|---|---|\n> cité\n---";
        assert_eq!(blocks(text), vec![
            Block::Heading("Titre".into()),
            Block::Paragraph("Un paragraphe\nsur deux lignes.".into()),
            Block::ListItem { marker: "•".into(), depth: 0, text: "un".into() },
            Block::ListItem { marker: "•".into(), depth: 1, text: "deux".into() },
            Block::ListItem { marker: "3.".into(), depth: 0, text: "trois".into() },
            Block::Code("fn main() {}".into()),
            Block::Code("| a | b |\n|---|---|".into()),
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
    fn hashes_without_space_are_text() {
        assert_eq!(blocks("#123 corrigé"), vec![Block::Paragraph("#123 corrigé".into())]);
    }
}
