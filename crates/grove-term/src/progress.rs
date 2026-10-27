//! Barre de progression du terminal (`ESC ] 9 ; 4 ; état ; valeur ST`), venue de
//! ConEmu et reprise par Windows Terminal, Ghostty et iTerm2. Claude Code s'en
//! sert pendant qu'il travaille.
//!
//! Le parseur d'alacritty ignore cette séquence : [`ProgressScanner`] la repère
//! dans le flux du PTY sans le modifier.

/// État de la barre affichée par le programme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Avancement connu, de 0 à 100.
    Normal(u8),
    Error(u8),
    /// En pause ou avertissement.
    Paused(u8),
    /// Avancement inconnu.
    Indeterminate,
}

impl Progress {
    /// Avancement de 0 à 100, `None` s'il est inconnu.
    pub fn value(self) -> Option<u8> {
        match self {
            Self::Normal(v) | Self::Error(v) | Self::Paused(v) => Some(v),
            Self::Indeterminate => None,
        }
    }
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const PREFIX: &[u8] = b"\x1b]9;4;";
/// Au-delà, ce n'est plus une séquence de progression : on l'abandonne.
const MAX_PARAMS: usize = 16;

#[derive(Default)]
pub(crate) struct ProgressScanner {
    /// Octets du préfixe déjà reconnus.
    matched: usize,
    /// Paramètres en cours (`état ; valeur`), après le préfixe.
    params: Option<Vec<u8>>,
    /// `ESC` reçu dans les paramètres : début possible de `ESC \`.
    escape: bool,
    current: Option<Progress>,
}

impl ProgressScanner {
    /// Lit `input` et renvoie le nouvel état de la barre s'il a changé.
    pub fn scan(&mut self, input: &[u8]) -> Option<Option<Progress>> {
        let before = self.current;
        for &byte in input {
            let Some(params) = &mut self.params else {
                self.prefix(byte);
                continue;
            };
            if self.escape {
                self.escape = false;
                if byte == b'\\' {
                    self.finish();
                } else {
                    // `ESC` suivi d'autre chose : la séquence est interrompue, une autre commence.
                    self.params = None;
                    self.matched = 1;
                    self.prefix(byte);
                }
            } else if byte == BEL {
                self.finish();
            } else if byte == ESC {
                self.escape = true;
            } else if params.len() < MAX_PARAMS && (byte.is_ascii_digit() || byte == b';') {
                params.push(byte);
            } else {
                self.params = None;
            }
        }
        (self.current != before).then_some(self.current)
    }

    fn prefix(&mut self, byte: u8) {
        if byte == PREFIX[self.matched] {
            self.matched += 1;
            if self.matched == PREFIX.len() {
                self.matched = 0;
                self.params = Some(Vec::new());
            }
        } else {
            self.matched = usize::from(byte == ESC);
        }
    }

    fn finish(&mut self) {
        let params = self.params.take().unwrap_or_default();
        self.current = parse(&params, self.current);
    }
}

/// `état ; valeur` → nouvel état. Sans valeur, l'erreur et la pause gardent la précédente.
fn parse(params: &[u8], current: Option<Progress>) -> Option<Progress> {
    let text = std::str::from_utf8(params).unwrap_or_default();
    let mut parts = text.split(';');
    let state = parts.next().unwrap_or_default();
    let value = parts.next().and_then(|v| v.parse::<u32>().ok()).map(|v| v.min(100) as u8);
    let previous = current.and_then(Progress::value).unwrap_or(0);
    match state {
        "" | "0" => None,
        "1" => Some(Progress::Normal(value.unwrap_or(0))),
        "2" => Some(Progress::Error(value.unwrap_or(previous))),
        "3" => Some(Progress::Indeterminate),
        "4" => Some(Progress::Paused(value.unwrap_or(previous))),
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_all(chunks: &[&[u8]]) -> Vec<Option<Progress>> {
        let mut scanner = ProgressScanner::default();
        chunks.iter().filter_map(|chunk| scanner.scan(chunk)).collect()
    }

    #[test]
    fn sequences_of_claude_code() {
        let updates = scan_all(&[
            b"texte\x1b]9;4;3;\x07suite",
            b"\x1b]9;4;1;42\x07",
            b"\x1b]9;4;2;\x1b\\",
            b"\x1b]9;4;0;\x07",
        ]);
        assert_eq!(updates, [
            Some(Progress::Indeterminate),
            Some(Progress::Normal(42)),
            Some(Progress::Error(42)),
            None,
        ]);
    }

    #[test]
    fn split_across_reads_and_repeated() {
        let updates = scan_all(&[b"\x1b]9", b";4;1;", b"150", b"\x1b", b"\\", b"\x1b]9;4;1;100\x07"]);
        assert_eq!(updates, [Some(Progress::Normal(100))], "valeur bornée, répétition ignorée");
    }

    #[test]
    fn other_sequences_are_ignored() {
        let updates = scan_all(&[
            b"\x1b]9;notification\x07",
            b"\x1b]0;titre 9;4;1;5\x07",
            b"\x1b\x1b]9;4;1;7\x07",
            b"\x1b]9;4;1;7 texte\x07",
        ]);
        assert_eq!(updates, [Some(Progress::Normal(7))]);
    }
}
