//! Protocole graphique de kitty, dans le sous-ensemble qu'utilisent Claude Code
//! et `kitten icat --unicode-placeholder` : transmission d'images (PNG ou pixels
//! bruts ; en ligne, par fichier ou par mémoire partagée), placements virtuels
//! (`U=1`) affichés par des caractères U+10EEEE, suppression.
//!
//! Le parseur d'alacritty ignore les séquences APC : [`ApcFilter`] les retire du
//! flux du PTY avant lui et transmet les commandes `ESC _ G … ESC \` à [`Graphics`].
//! Les placements classiques (image posée au curseur, sans placeholder) sont
//! acceptés mais pas affichés.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use alacritty_terminal::vte::ansi::Color;
use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};

/// Caractère qui tient la place d'une cellule d'image.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Taille maximale d'une commande (données en base64 comprises).
const MAX_COMMAND: usize = 64 << 20;
/// Taille maximale d'un fichier ou d'un segment de mémoire partagée lu.
const MAX_FILE: u64 = 128 << 20;
/// Nombre maximal de pixels d'une image décodée.
const MAX_PIXELS: u64 = 40_000_000;
/// Mémoire maximale des images gardées ; au-delà, les plus anciennes partent.
const MAX_STORED: usize = 320 << 20;

/// Diacritiques qui codent ligne, colonne et octet de poids fort de l'identifiant
/// d'un placeholder (table `rowcolumn-diacritics` de la spécification de kitty).
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C, 0x0350,
    0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B,
    0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595,
    0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9, 0x05AB, 0x05AC,
    0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658, 0x0659,
    0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0,
    0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A,
    0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE,
    0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F,
    0x0820, 0x0821, 0x0822, 0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951,
    0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75,
    0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5,
    0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7,
    0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4,
    0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7,
    0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7,
    0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4,
    0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA, 0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C,
    0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9,
    0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8,
    0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38,
    0x1D185, 0x1D186, 0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243,
    0x1D244,
];

/// Les données en base64 peuvent arriver avec ou sans `=` final.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Image transmise par le programme, en RGBA.
#[derive(Debug)]
pub struct Image {
    pub id: u32,
    /// Change à chaque nouvelle transmission : clé de cache pour l'UI.
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    /// Zone du placement virtuel, en cellules ; 0 : déduite de la taille de l'image.
    pub columns: u32,
    pub rows: u32,
}

/// Cellule occupée par un morceau d'image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placeholder {
    pub image: u32,
    /// Ligne et colonne de la cellule dans la zone du placement.
    pub row: u32,
    pub column: u32,
}

/// Décode un placeholder à partir de la couleur du texte (identifiant) et des
/// diacritiques (ligne, colonne, octet de poids fort). Ce qui manque est déduit
/// de la cellule de gauche, comme le prévoit la spécification.
pub(crate) fn placeholder(fg: Color, zerowidth: &[char], left: Option<Placeholder>) -> Option<Placeholder> {
    let low = match fg {
        Color::Spec(rgb) => (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32,
        Color::Indexed(index) => index as u32,
        Color::Named(_) => return None,
    };
    let index = |n: usize| zerowidth.get(n).and_then(|&c| DIACRITICS.iter().position(|&d| d == c as u32));
    let left = left.filter(|l| l.image & 0xFF_FFFF == low);
    let high = match index(2) {
        Some(high) => high as u32,
        None => left.map_or(0, |l| l.image >> 24),
    };
    let image = high << 24 | low;
    let left = left.filter(|l| l.image == image);
    let row = match index(0) {
        Some(row) => row as u32,
        None => left.map_or(0, |l| l.row),
    };
    let column = match index(1) {
        Some(column) => column as u32,
        None => left.filter(|l| l.row == row).map_or(0, |l| l.column + 1),
    };
    (image != 0).then_some(Placeholder { image, row, column })
}

/// Retire du flux du PTY les commandes graphiques (`ESC _ G … ESC \`) ; le reste
/// passe tel quel, y compris les autres APC, que le parseur ignorera.
#[derive(Default)]
pub(crate) struct ApcFilter {
    state: State,
    command: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// `ESC` reçu.
    Escape,
    /// `ESC _` reçu.
    Apc,
    /// Dans une commande graphique.
    Command,
    /// `ESC` reçu dans une commande graphique.
    CommandEscape,
    /// Commande trop longue : ignorée jusqu'à sa fin.
    Discard,
    DiscardEscape,
}

const ESC: u8 = 0x1b;

impl ApcFilter {
    /// Copie `input` dans `out` sans les commandes graphiques, passées à `on_command`.
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>, mut on_command: impl FnMut(&[u8])) {
        let mut i = 0;
        while i < input.len() {
            if self.state == State::Ground {
                // Chemin courant : du texte sans séquence, copié d'un bloc.
                let end = input[i..].iter().position(|&b| b == ESC).map_or(input.len(), |p| i + p);
                out.extend_from_slice(&input[i..end]);
                if end == input.len() {
                    return;
                }
                self.state = State::Escape;
                i = end + 1;
                continue;
            }
            let byte = input[i];
            i += 1;
            self.state = match self.state {
                State::Ground => unreachable!(),
                State::Escape if byte == b'_' => State::Apc,
                State::Escape => {
                    out.push(ESC);
                    if byte == ESC {
                        State::Escape
                    } else {
                        out.push(byte);
                        State::Ground
                    }
                }
                State::Apc if byte == b'G' => {
                    self.command.clear();
                    State::Command
                }
                State::Apc => {
                    out.extend_from_slice(&[ESC, b'_']);
                    if byte == ESC {
                        State::Escape
                    } else {
                        out.push(byte);
                        State::Ground
                    }
                }
                State::Command if byte == ESC => State::CommandEscape,
                State::Command if self.command.len() >= MAX_COMMAND => {
                    self.command = Vec::new();
                    State::Discard
                }
                State::Command => {
                    self.command.push(byte);
                    State::Command
                }
                State::CommandEscape if byte == b'\\' => {
                    on_command(&self.command);
                    self.command.clear();
                    State::Ground
                }
                State::CommandEscape => {
                    self.command.push(ESC);
                    if byte == ESC {
                        State::CommandEscape
                    } else {
                        self.command.push(byte);
                        State::Command
                    }
                }
                State::Discard if byte == ESC => State::DiscardEscape,
                State::Discard => State::Discard,
                State::DiscardEscape if byte == b'\\' => State::Ground,
                State::DiscardEscape if byte == ESC => State::DiscardEscape,
                State::DiscardEscape => State::Discard,
            };
        }
    }
}

/// Clés de contrôle d'une commande : `a=T,f=100,i=31`.
struct Control(Vec<(u8, String)>);

impl Control {
    fn parse(text: &[u8]) -> Self {
        let pairs = text
            .split(|&b| b == b',')
            .filter_map(|pair| {
                let (&key, rest) = pair.split_first()?;
                let value = rest.strip_prefix(b"=")?;
                Some((key, String::from_utf8_lossy(value).into_owned()))
            })
            .collect();
        Self(pairs)
    }

    fn get(&self, key: u8) -> Option<&str> {
        self.0.iter().rev().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    fn number(&self, key: u8) -> u32 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    fn letter(&self, key: u8, default: u8) -> u8 {
        self.get(key).and_then(|v| v.bytes().next()).unwrap_or(default)
    }
}

/// Transmission en plusieurs morceaux (`m=1`) en cours.
struct Chunked {
    control: Control,
    data: Vec<u8>,
}

/// Images reçues et état du protocole, partagés entre le thread d'E/S et l'UI.
#[derive(Default)]
pub(crate) struct Graphics {
    images: HashMap<u32, Arc<Image>>,
    chunked: Option<Chunked>,
    next_generation: u64,
}

impl Graphics {
    pub fn image(&self, id: u32) -> Option<Arc<Image>> {
        self.images.get(&id).cloned()
    }

    /// Exécute une commande (le texte entre `ESC _G` et `ESC \`) ; renvoie la
    /// réponse à écrire au programme, s'il en attend une.
    pub fn command(&mut self, raw: &[u8]) -> Option<String> {
        let (control, payload) = match raw.iter().position(|&b| b == b';') {
            Some(at) => (&raw[..at], &raw[at + 1..]),
            None => (raw, &[][..]),
        };
        let control = Control::parse(control);

        // Les morceaux suivants ne portent que `m` (et `q`) ; une commande
        // complète abandonne une transmission restée en plan.
        if let Some(mut chunked) = self.chunked.take()
            && control.get(b'a').is_none()
            && control.get(b'i').is_none()
        {
            chunked.data.extend_from_slice(payload);
            if control.number(b'm') == 1 {
                self.chunked = Some(chunked);
                return None;
            }
            if let Some(quiet) = control.get(b'q') {
                chunked.control.0.push((b'q', quiet.to_owned()));
            }
            return self.complete(&chunked.control, &chunked.data);
        }
        if control.number(b'm') == 1 {
            self.chunked = Some(Chunked { control, data: payload.to_vec() });
            return None;
        }
        self.complete(&control, payload)
    }

    fn complete(&mut self, control: &Control, payload: &[u8]) -> Option<String> {
        let id = control.number(b'i');
        let result = match control.letter(b'a', b't') {
            b'q' => decode(control, payload).map(|_| ()),
            b't' | b'T' => decode(control, payload).map(|(width, height, rgba)| {
                self.store(control, Image {
                    id,
                    generation: 0,
                    width,
                    height,
                    rgba: Arc::new(rgba),
                    columns: 0,
                    rows: 0,
                });
            }),
            b'p' => match self.images.get(&id) {
                Some(image) => {
                    let image = Image { rgba: image.rgba.clone(), ..**image };
                    self.place(control, image);
                    Ok(())
                }
                None => Err("ENOENT:image inconnue".to_owned()),
            },
            b'd' => {
                match control.letter(b'd', b'a') {
                    b'a' | b'A' => self.images.clear(),
                    b'i' | b'I' => {
                        self.images.remove(&id);
                    }
                    // Suppressions par position : les placements classiques ne sont pas affichés.
                    _ => {}
                }
                return None;
            }
            _ => return None,
        };
        reply(id, control.number(b'q'), result)
    }

    /// Range une image transmise ; `a=T,U=1` crée en plus son placement virtuel.
    fn store(&mut self, control: &Control, mut image: Image) {
        if image.id == 0 {
            return;
        }
        image.generation = self.next_generation;
        self.next_generation += 1;
        if control.letter(b'a', b't') == b'T' {
            self.place(control, image);
        } else {
            self.images.insert(image.id, Arc::new(image));
        }
        self.evict();
    }

    fn place(&mut self, control: &Control, mut image: Image) {
        if control.number(b'U') == 1 {
            image.columns = control.number(b'c');
            image.rows = control.number(b'r');
        }
        self.images.insert(image.id, Arc::new(image));
    }

    /// Libère les images les plus anciennes au-delà de [`MAX_STORED`].
    fn evict(&mut self) {
        let mut total: usize = self.images.values().map(|i| i.rgba.len()).sum();
        while total > MAX_STORED {
            let Some(oldest) = self.images.values().min_by_key(|i| i.generation).map(|i| i.id) else {
                break;
            };
            if let Some(image) = self.images.remove(&oldest) {
                total -= image.rgba.len();
            }
        }
    }
}

/// Réponse `ESC _G i=<id>;OK ESC \` ou l'erreur, selon le niveau de silence `q`.
fn reply(id: u32, quiet: u32, result: Result<(), String>) -> Option<String> {
    if id == 0 {
        return None;
    }
    let message = match result {
        Ok(()) if quiet >= 1 => return None,
        Ok(()) => "OK".to_owned(),
        Err(_) if quiet >= 2 => return None,
        Err(error) => error,
    };
    Some(format!("\x1b_Gi={id};{message}\x1b\\"))
}

/// Lit les données d'une commande (support `t`, compression `o`) et les décode
/// selon leur format `f` : largeur, hauteur, pixels RGBA.
fn decode(control: &Control, payload: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let payload = BASE64.decode(payload).map_err(|_| "EINVAL:base64 invalide".to_owned())?;
    let bytes = match control.letter(b't', b'd') {
        b'd' => payload,
        medium @ (b'f' | b't' | b's') => {
            let name = String::from_utf8(payload).map_err(|_| "EINVAL:nom invalide".to_owned())?;
            read_medium(medium, &name, control.number(b'O'), control.number(b'S'))
                .map_err(|err| format!("EBADF:{err}"))?
        }
        _ => return Err("EINVAL:support de transmission inconnu".into()),
    };
    let bytes = if control.letter(b'o', 0) == b'z' {
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&bytes[..])
            .take(MAX_PIXELS * 4)
            .read_to_end(&mut inflated)
            .map_err(|_| "EINVAL:compression invalide".to_owned())?;
        inflated
    } else {
        bytes
    };

    match control.number(b'f') {
        100 => {
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|err| format!("EBADPNG:{err}"))?;
            check_size(image.width(), image.height())?;
            Ok((image.width(), image.height(), image.into_rgba8().into_raw()))
        }
        format @ (0 | 24 | 32) => {
            let (width, height) = (control.number(b's'), control.number(b'v'));
            check_size(width, height)?;
            let channels = if format == 24 { 3 } else { 4 };
            let len = width as usize * height as usize * channels;
            let pixels = bytes.get(..len).ok_or_else(|| "ENODATA:données insuffisantes".to_owned())?;
            let rgba = if channels == 4 {
                pixels.to_vec()
            } else {
                pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 0xff]).collect()
            };
            Ok((width, height, rgba))
        }
        _ => Err("EINVAL:format inconnu".into()),
    }
}

fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return Err("EINVAL:dimensions invalides".into());
    }
    Ok(())
}

/// Contenu d'un fichier (`f`), d'un fichier temporaire supprimé ensuite (`t`) ou
/// d'un segment de mémoire partagée POSIX supprimé ensuite (`s`).
fn read_medium(medium: u8, name: &str, offset: u32, size: u32) -> io::Result<Vec<u8>> {
    let path = if medium == b's' {
        let name = name.trim_start_matches('/');
        if name.is_empty() || name.contains('/') {
            return Err(io::Error::other("nom de mémoire partagée invalide"));
        }
        Path::new("/dev/shm").join(name)
    } else {
        Path::new(name).to_owned()
    };
    let bytes = read_regular(&path, offset, size);
    // Le programme confie ces données au terminal, qui doit les libérer. Un
    // fichier temporaire n'est supprimé que s'il en a bien l'air (spécification).
    let delete = match medium {
        b's' => true,
        b't' => {
            (path.starts_with(std::env::temp_dir()) || path.starts_with("/dev/shm"))
                && path.file_name().is_some_and(|n| n.to_string_lossy().contains("tty-graphics-protocol"))
        }
        _ => false,
    };
    if delete {
        let _ = std::fs::remove_file(&path);
    }
    bytes
}

/// Lit un fichier ordinaire : jamais un périphérique ou un tube, qui pourraient
/// bloquer le thread d'E/S ou ne jamais finir.
fn read_regular(path: &Path, offset: u32, size: u32) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("pas un fichier ordinaire"));
    }
    let available = metadata.len().saturating_sub(offset as u64);
    let len = if size == 0 { available } else { (size as u64).min(available) };
    if len > MAX_FILE {
        return Err(io::Error::other("fichier trop gros"));
    }
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::vte::ansi::Rgb;
    use base64::engine::general_purpose;

    use super::*;

    fn filter(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut filter = ApcFilter::default();
        let (mut out, mut commands) = (Vec::new(), Vec::new());
        for chunk in chunks {
            filter.feed(chunk, &mut out, |c| commands.push(c.to_vec()));
        }
        (out, commands)
    }

    #[test]
    fn filter_extracts_graphics_commands_across_reads() {
        let (out, commands) = filter(&[b"ab\x1b[1mc\x1b_Ga=q,i=1;AA", b"AA\x1b", b"\\d\x1b_Xautre\x1b\\e"]);
        assert_eq!(out, b"ab\x1b[1mcd\x1b_Xautre\x1b\\e");
        assert_eq!(commands, vec![b"a=q,i=1;AAAA".to_vec()]);
    }

    #[test]
    fn filter_keeps_lone_escapes() {
        let (out, commands) = filter(&[b"\x1b", b"\x1b[A\x1b_", b"x"]);
        assert_eq!(out, b"\x1b\x1b[A\x1b_x");
        assert!(commands.is_empty());
    }

    fn rgba_command(control: &str, pixels: &[u8]) -> Vec<u8> {
        format!("{control};{}", general_purpose::STANDARD.encode(pixels)).into_bytes()
    }

    #[test]
    fn query_transmit_place_and_delete() {
        let mut graphics = Graphics::default();
        let reply = graphics.command(&rgba_command("i=31,s=1,v=1,a=q,t=d,f=24", &[0, 0, 0]));
        assert_eq!(reply.as_deref(), Some("\x1b_Gi=31;OK\x1b\\"));
        assert!(graphics.image(31).is_none(), "une requête ne garde pas l'image");

        let pixels = [255, 0, 0, 255, 0, 255, 0, 255];
        let reply = graphics.command(&rgba_command("a=T,U=1,q=2,f=32,s=2,v=1,i=7,c=4,r=2", &pixels));
        assert_eq!(reply, None, "q=2 : pas de réponse");
        let image = graphics.image(7).unwrap();
        assert_eq!((image.width, image.height, image.columns, image.rows), (2, 1, 4, 2));
        assert_eq!(*image.rgba, pixels);

        graphics.command(b"a=d,d=I,i=7,q=2");
        assert!(graphics.image(7).is_none());
    }

    #[test]
    fn chunked_png_transmission() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let data = general_purpose::STANDARD.encode(&png);
        let (first, rest) = data.split_at(8);
        let mut graphics = Graphics::default();
        assert_eq!(graphics.command(format!("a=T,U=1,f=100,i=5,c=3,r=1,m=1;{first}").as_bytes()), None);
        let reply = graphics.command(format!("m=0;{rest}").as_bytes());
        assert_eq!(reply.as_deref(), Some("\x1b_Gi=5;OK\x1b\\"));
        let image = graphics.image(5).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.rgba[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn file_transmission_and_errors() {
        let dir = std::env::temp_dir().join(format!("grove-graphics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pixels.rgb");
        std::fs::write(&path, [9, 8, 7]).unwrap();
        let name = general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes());

        let mut graphics = Graphics::default();
        let reply = graphics.command(format!("a=t,t=f,f=24,s=1,v=1,i=3;{name}").as_bytes());
        assert_eq!(reply.as_deref(), Some("\x1b_Gi=3;OK\x1b\\"));
        assert_eq!(*graphics.image(3).unwrap().rgba, [9, 8, 7, 255]);
        assert!(path.exists(), "t=f ne supprime pas le fichier");

        let missing = general_purpose::STANDARD.encode(b"/nonexistent/x.png");
        let reply = graphics.command(format!("a=q,t=f,f=100,i=4;{missing}").as_bytes()).unwrap();
        assert!(reply.starts_with("\x1b_Gi=4;EBADF:"), "{reply:?}");
        let device = general_purpose::STANDARD.encode(b"/dev/zero");
        let reply = graphics.command(format!("a=q,t=f,f=100,i=4;{device}").as_bytes()).unwrap();
        assert!(reply.contains("EBADF"), "un périphérique n'est jamais lu : {reply:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decodes_placeholders() {
        let fg = Color::Spec(Rgb { r: 0, g: 0, b: 42 });
        let d = |n: usize| char::from_u32(DIACRITICS[n]).unwrap();
        let first = placeholder(fg, &[d(1), d(0)], None).unwrap();
        assert_eq!(first, Placeholder { image: 42, row: 1, column: 0 });
        // Sans diacritiques : suite de la cellule de gauche.
        let next = placeholder(fg, &[], Some(first)).unwrap();
        assert_eq!(next, Placeholder { image: 42, row: 1, column: 1 });
        // Troisième diacritique : octet de poids fort de l'identifiant.
        let high = placeholder(Color::Indexed(5), &[d(0), d(0), d(2)], None).unwrap();
        assert_eq!(high.image, 2 << 24 | 5);
        assert_eq!(placeholder(Color::Named(alacritty_terminal::vte::ansi::NamedColor::Foreground), &[], None), None);
    }
}
