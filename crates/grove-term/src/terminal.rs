use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use anyhow::{Context, Result};

use crate::colors::Palette;
use crate::graphics::{self, Graphics, Image, Placeholder};
use crate::links::{self, Link};
use crate::pty::GraphicsPty;

pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::term::cell::Flags as CellFlags;
pub use alacritty_terminal::vte::ansi::{CursorShape, Rgb};

/// Événements remontés à l'UI. Le callback est appelé depuis le thread d'E/S.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// Nouveau contenu à afficher.
    Wakeup,
    /// Titre défini par le programme (OSC 0/2). `None` : titre réinitialisé.
    Title(Option<String>),
    Bell,
    /// Le processus du PTY s'est terminé.
    Exited(Option<i32>),
}

#[derive(Clone, Debug)]
pub struct TerminalOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub scrollback: usize,
    pub columns: u16,
    pub lines: u16,
}

impl TerminalOptions {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            scrollback: 10_000,
            columns: 80,
            lines: 24,
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    window_size: Arc<Mutex<WindowSize>>,
    palette: Palette,
    /// Compilée une fois : la construction des automates coûte plus que la recherche.
    url_regex: Mutex<RegexSearch>,
    /// Images du protocole graphique de kitty, reçues par le thread d'E/S.
    graphics: Arc<Mutex<Graphics>>,
}

impl Terminal {
    pub fn spawn(
        options: TerminalOptions,
        on_event: impl Fn(TerminalEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut env = options.env;
        env.entry("TERM".into()).or_insert_with(|| "xterm-256color".into());
        env.entry("COLORTERM".into()).or_insert_with(|| "truecolor".into());
        let pty_options = tty::Options {
            shell: Some(tty::Shell::new(options.program, options.args)),
            working_directory: options.cwd,
            drain_on_exit: true,
            env,
        };

        let size = Size { columns: options.columns.max(1), lines: options.lines.max(1) };
        let pty = tty::new(&pty_options, size.window_size(), 0).context("création du PTY")?;

        let listener = Listener {
            sender: Arc::default(),
            window_size: Arc::new(Mutex::new(size.window_size())),
            on_event: Arc::new(on_event),
        };
        let config = term::Config { scrolling_history: options.scrollback, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let graphics = Arc::new(Mutex::new(Graphics::default()));
        let pty = GraphicsPty::new(pty, graphics.clone(), listener.sender.clone()).context("création du PTY")?;

        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)
            .context("démarrage de la boucle d'E/S du terminal")?;
        let sender = event_loop.channel();
        let _ = listener.sender.set(sender.clone());
        event_loop.spawn();

        Ok(Self {
            term,
            sender,
            window_size: listener.window_size,
            palette: Palette::default(),
            url_regex: Mutex::new(links::url_regex()),
            graphics,
        })
    }

    /// Envoie des octets au programme (saisie clavier, collage…).
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(bytes));
        }
    }

    /// Colle du texte, encadré par les séquences de bracketed paste si le programme les a activées.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            // Le texte collé ne doit pas pouvoir clore lui-même le bracketed paste.
            let text = text.replace("\x1b[201~", "");
            self.write(format!("\x1b[200~{text}\x1b[201~").into_bytes());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
    }

    /// Redimensionne la grille et le PTY (SIGWINCH).
    pub fn resize(&self, columns: u16, lines: u16, cell_width: u16, cell_height: u16) {
        let size = Size { columns: columns.max(1), lines: lines.max(1) };
        let window_size = WindowSize { cell_width, cell_height, ..size.window_size() };
        {
            let mut current = self.window_size.lock().unwrap();
            if (current.num_cols, current.num_lines, current.cell_width, current.cell_height)
                == (columns.max(1), lines.max(1), cell_width, cell_height)
            {
                return;
            }
            *current = window_size;
        }
        self.graphics.lock().unwrap().set_cell_size(cell_width as u32, cell_height as u32);
        self.term.lock().resize(size);
        let _ = self.sender.send(Msg::Resize(window_size));
    }

    /// Fait défiler le scrollback : positif vers le haut (historique).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Lien (hyperlien OSC 8 ou URL du texte) sous la cellule visible (`row`, `column`).
    pub fn link_at(&self, row: usize, column: usize) -> Option<Link> {
        let term = self.term.lock();
        if row >= term.screen_lines() || column >= term.columns() {
            return None;
        }
        let offset = term.grid().display_offset() as i32;
        let point = Point::new(Line(row as i32 - offset), Column(column));
        links::link_at(&term, &mut self.url_regex.lock().unwrap(), point)
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Copie de l'écran visible, couleurs résolues, prête à dessiner.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let columns = term.columns();
        let lines = term.screen_lines();

        let mut cells = Vec::with_capacity(columns * lines);
        // Placeholder de la cellule de gauche : ce qui manque à une cellule d'image s'en déduit.
        let mut left: Option<(i32, Placeholder)> = None;
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let line = indexed.point.line.0;
            let placeholder = (cell.c == graphics::PLACEHOLDER)
                .then(|| {
                    let left = left.filter(|(row, _)| *row == line).map(|(_, p)| p);
                    graphics::placeholder(cell.fg, cell.zerowidth().unwrap_or_default(), left)
                })
                .flatten();
            left = placeholder.map(|p| (line, p));
            let mut fg = self.palette.resolve(cell.fg, content.colors, cell.flags);
            let mut bg = self.palette.resolve(cell.bg, content.colors, cell.flags - Flags::BOLD - Flags::DIM);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
            cells.push(RenderCell {
                row: (indexed.point.line.0 + offset) as usize,
                column: indexed.point.column.0,
                c: cell.c,
                zerowidth: cell.zerowidth().map(|z| z.iter().collect()),
                fg,
                bg,
                flags: cell.flags,
                selected,
                placeholder,
            });
        }

        let mut images = HashMap::new();
        {
            let graphics = self.graphics.lock().unwrap();
            for id in cells.iter().filter_map(|c| c.placeholder).map(|p| p.image) {
                if let std::collections::hash_map::Entry::Vacant(entry) = images.entry(id)
                    && let Some(image) = graphics.image(id)
                {
                    entry.insert(image);
                }
            }
        }

        let cursor = content.cursor;
        let cursor_row = cursor.point.line.0 + offset;
        Snapshot {
            columns,
            lines,
            cells,
            cursor: (cursor_row >= 0 && (cursor_row as usize) < lines)
                .then_some((cursor_row as usize, cursor.point.column.0)),
            cursor_shape: cursor.shape,
            display_offset: content.display_offset,
            background: content.colors[alacritty_terminal::vte::ansi::NamedColor::Background]
                .unwrap_or(self.palette.background),
            cursor_color: self.palette.cursor,
            mode: content.mode,
            images,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

/// Une cellule visible, prête à dessiner.
#[derive(Clone, Debug)]
pub struct RenderCell {
    pub row: usize,
    pub column: usize,
    pub c: char,
    /// Caractères combinants (accents, ZWJ…).
    pub zerowidth: Option<String>,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: Flags,
    pub selected: bool,
    /// Cellule d'image (caractère U+10EEEE du protocole graphique de kitty).
    pub placeholder: Option<Placeholder>,
}

pub struct Snapshot {
    pub columns: usize,
    pub lines: usize,
    pub cells: Vec<RenderCell>,
    /// (ligne, colonne) du curseur s'il est dans la zone visible.
    pub cursor: Option<(usize, usize)>,
    pub cursor_shape: CursorShape,
    pub display_offset: usize,
    pub background: Rgb,
    pub cursor_color: Rgb,
    pub mode: TermMode,
    /// Images dont des cellules sont visibles, par identifiant.
    pub images: HashMap<u32, Arc<Image>>,
}

impl Snapshot {
    /// Texte d'une ligne visible, sans les espaces de fin.
    pub fn line_text(&self, row: usize) -> String {
        let mut text: String = self.cells.iter().filter(|c| c.row == row).map(|c| c.c).collect();
        text.truncate(text.trim_end().len());
        text
    }
}

#[derive(Clone, Copy)]
struct Size {
    columns: u16,
    lines: u16,
}

impl Size {
    fn window_size(self) -> WindowSize {
        WindowSize { num_lines: self.lines, num_cols: self.columns, cell_width: 1, cell_height: 1 }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines as usize
    }

    fn screen_lines(&self) -> usize {
        self.lines as usize
    }

    fn columns(&self) -> usize {
        self.columns as usize
    }
}

#[derive(Clone)]
struct Listener {
    /// Renseigné juste après la création de la boucle d'E/S, pour répondre
    /// aux requêtes du programme (rapport de position du curseur…).
    sender: Arc<OnceLock<EventLoopSender>>,
    /// Dernière taille connue, pour répondre aux requêtes de taille (CSI 14 t).
    window_size: Arc<Mutex<WindowSize>>,
    on_event: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
}

impl Listener {
    fn reply(&self, text: String) {
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Msg::Input(text.into_bytes().into()));
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let event = match event {
            Event::Wakeup => TerminalEvent::Wakeup,
            Event::Title(title) => TerminalEvent::Title(Some(title)),
            Event::ResetTitle => TerminalEvent::Title(None),
            Event::Bell => TerminalEvent::Bell,
            Event::ChildExit(status) => TerminalEvent::Exited(status.code()),
            Event::PtyWrite(text) => return self.reply(text),
            Event::TextAreaSizeRequest(format) => {
                let size = *self.window_size.lock().unwrap();
                return self.reply(format(size));
            }
            _ => return,
        };
        (self.on_event)(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_for(terminal: &Terminal, mut pred: impl FnMut(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = terminal.snapshot();
            if pred(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "délai dépassé");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn runs_a_command_and_reports_exit() {
        let (tx, rx) = mpsc::channel();
        let mut options = TerminalOptions::new("/bin/sh");
        options.args = vec!["-c".into(), "printf 'hello grove'; exit 3".into()];
        let terminal = Terminal::spawn(options, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let exit = loop {
            match rx.recv_timeout(deadline - Instant::now()).expect("pas d'événement de fin") {
                TerminalEvent::Exited(code) => break code,
                _ => continue,
            }
        };
        assert_eq!(exit, Some(3));
        assert_eq!(terminal.snapshot().line_text(0), "hello grove");
    }

    #[test]
    fn colors_and_input() {
        let mut options = TerminalOptions::new("/bin/sh");
        options.args = vec!["-c".into(), "read x; printf '\\033[31mR\\033[0m%s' \"$x\"; sleep 5".into()];
        let terminal = Terminal::spawn(options, |_| {}).unwrap();

        terminal.write(&b"ok\r"[..]);
        let snapshot = wait_for(&terminal, |s| (0..s.lines).any(|r| s.line_text(r).contains("Rok")));
        let r = snapshot.cells.iter().find(|c| c.c == 'R').unwrap();
        assert_eq!(r.fg, terminal.palette().ansi[1]);
    }

    #[test]
    fn detects_urls_and_hyperlinks() {
        let mut options = TerminalOptions::new("/bin/sh");
        options.args = vec![
            "-c".into(),
            r"printf 'voir https://grove.dev/doc. ok\n\033]8;;https://example.com/x\033\\lien\033]8;;\033\\ fin'; sleep 5".into(),
        ];
        let terminal = Terminal::spawn(options, |_| {}).unwrap();
        wait_for(&terminal, |s| s.line_text(1).contains("fin"));

        let link = terminal.link_at(0, 10).expect("URL détectée");
        assert_eq!(link.uri, "https://grove.dev/doc");
        assert_eq!((link.start, link.end), ((0, 5), (0, 25)));
        assert_eq!(terminal.link_at(0, 26), None, "le point final n'en fait pas partie");
        assert_eq!(terminal.link_at(0, 1), None);

        let hyperlink = terminal.link_at(1, 2).expect("hyperlien OSC 8");
        assert_eq!(hyperlink.uri, "https://example.com/x");
        assert_eq!((hyperlink.start, hyperlink.end), ((1, 0), (1, 3)));
        assert_eq!(terminal.link_at(1, 5), None);
    }

    #[test]
    fn kitty_graphics_query_and_placeholders() {
        // Requête de détection, réponse relue par le programme ; puis une image
        // 1×1 (id 42) et deux cellules U+10EEEE, ligne 0, colonnes 0 et 1.
        let script = concat!(
            r"stty raw -echo; printf '\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\033\\'; ",
            r#"r=$(dd bs=1 count=12 2>/dev/null); printf '%s\n' "$r" | tr '\033' E; "#,
            r"printf '\033_Ga=T,U=1,q=2,f=32,s=1,v=1,i=42,c=2,r=1;/wAA/w==\033\\'; ",
            r"printf '\033[38;2;0;0;42m\364\216\273\256\314\205\314\205\364\216\273\256\033[0m fin'; sleep 5",
        );
        let mut options = TerminalOptions::new("/bin/sh");
        options.args = vec!["-c".into(), script.into()];
        let terminal = Terminal::spawn(options, |_| {}).unwrap();

        let snapshot = wait_for(&terminal, |s| (0..s.lines).any(|r| s.line_text(r).contains("fin")));
        assert!(snapshot.line_text(0).contains("E_Gi=31;OKE\\"), "réponse : {:?}", snapshot.line_text(0));
        let cells: Vec<_> = snapshot.cells.iter().filter_map(|c| c.placeholder).collect();
        assert_eq!(cells, [
            Placeholder { image: 42, row: 0, column: 0 },
            Placeholder { image: 42, row: 0, column: 1 },
        ]);
        let image = &snapshot.images[&42];
        assert_eq!((image.width, image.height, image.columns, image.rows), (1, 1, 2, 1));
        assert_eq!(*image.rgba, [255, 0, 0, 255]);
    }

    #[test]
    fn resize_changes_grid() {
        let mut options = TerminalOptions::new("/bin/sh");
        options.args = vec!["-c".into(), "sleep 5".into()];
        let terminal = Terminal::spawn(options, |_| {}).unwrap();
        terminal.resize(120, 40, 8, 16);
        let snapshot = terminal.snapshot();
        assert_eq!((snapshot.columns, snapshot.lines), (120, 40));
    }
}
