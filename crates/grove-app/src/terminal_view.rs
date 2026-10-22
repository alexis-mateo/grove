use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    App, AppContext, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    Font, FontStyle, FontWeight, Hsla, KeyDownEvent, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollDelta, ScrollWheelEvent,
    SharedString, StrikethroughStyle, Task, TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill,
    font, point, prelude::*, px, size,
};
use grove_core::Config;
use grove_term::{
    CellFlags, CursorShape, Link, RenderCell, Rgb, Snapshot, TermMode, Terminal, TerminalEvent,
    TerminalOptions,
};

use crate::images::ImageCache;
use crate::mouse::{self, Action};
use crate::{builtin_glyphs, fonts, keys};

/// Hauteur de ligne relative à la taille de police.
const LINE_HEIGHT: f32 = 1.3;

pub struct TerminalView {
    terminal: Arc<Terminal>,
    focus_handle: FocusHandle,
    font: Font,
    font_size: Pixels,
    title: Option<String>,
    exited: Option<Option<i32>>,
    /// Défilement en pixels pas encore converti en lignes.
    pending_scroll: f32,
    /// Géométrie de la grille au dernier rendu : conversion pixels → cellules.
    layout: Rc<Cell<GridLayout>>,
    /// Dernière cellule signalée en mouvement, pour ne pas répéter le même report.
    last_motion: Option<(usize, usize)>,
    /// Dernière position de la souris sur le terminal (survol des liens quand Ctrl change).
    mouse_position: Option<Point<Pixels>>,
    /// Lien sous la souris, Ctrl enfoncé : souligné, ouvert au clic.
    hovered_link: Option<Link>,
    /// Texte en cours de composition (touche morte, méthode de saisie), affiché au curseur.
    marked_text: Option<String>,
    /// Textures des images du protocole graphique de kitty.
    images: Rc<RefCell<ImageCache>>,
    _events: Task<()>,
}

#[derive(Clone, Copy, Default)]
struct GridLayout {
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    columns: usize,
    lines: usize,
    /// Curseur au dernier rendu : position de la fenêtre de la méthode de saisie.
    cursor: Option<(usize, usize)>,
}

/// Émis quand le processus du terminal s'est terminé.
pub struct Exited;

impl gpui::EventEmitter<Exited> for TerminalView {}

impl TerminalView {
    /// Lance le programme décrit par `options` et crée la vue. Échoue si le PTY ne peut pas être créé.
    pub fn spawn(options: TerminalOptions, config: &Config, cx: &mut App) -> Result<Entity<Self>> {
        let (tx, rx) = smol::channel::unbounded();
        let terminal = Terminal::spawn(options, move |event| {
            let _ = tx.try_send(event);
        })?;
        Ok(cx.new(|cx| Self::new(terminal, rx, config, cx)))
    }

    fn new(
        terminal: Terminal,
        rx: smol::channel::Receiver<TerminalEvent>,
        config: &Config,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                // Regroupe les événements arrivés entre-temps en un seul rafraîchissement.
                let mut batch = vec![event];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                }
                let updated = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle_event(event, cx);
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        });

        Self {
            terminal: Arc::new(terminal),
            focus_handle: cx.focus_handle(),
            font: font(fonts::resolve_family(&config.font.family)),
            font_size: px(config.font.size),
            title: None,
            exited: None,
            pending_scroll: 0.0,
            layout: Rc::new(Cell::new(GridLayout {
                line_height: px(config.font.size * LINE_HEIGHT),
                ..Default::default()
            })),
            last_motion: None,
            mouse_position: None,
            hovered_link: None,
            marked_text: None,
            images: Rc::default(),
            _events: events,
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn handle_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup | TerminalEvent::Bell => {}
            TerminalEvent::Title(title) => self.title = title,
            TerminalEvent::Exited(code) => {
                self.exited = Some(code);
                cx.emit(Exited);
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;

        if m.control && m.shift && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.terminal.scroll_to_bottom();
                self.terminal.paste(&text);
            }
            cx.stop_propagation();
            return;
        }
        if m.shift && !m.control && (keystroke.key == "pageup" || keystroke.key == "pagedown") {
            let page = self.terminal.snapshot().lines as i32;
            self.terminal.scroll(if keystroke.key == "pageup" { page } else { -page });
            cx.notify();
            cx.stop_propagation();
            return;
        }

        let app_cursor = self.terminal.mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::to_esc(keystroke, app_cursor) {
            self.terminal.scroll_to_bottom();
            self.terminal.write(bytes);
            cx.stop_propagation();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => delta.y / self.layout.get().line_height,
        };
        self.pending_scroll += lines;
        let whole = self.pending_scroll.trunc();
        if whole == 0.0 {
            return;
        }
        self.pending_scroll -= whole;

        let mode = self.terminal.mode();
        let count = whole.abs() as usize;
        if self.reports_mouse(&event.modifiers) {
            // Le programme gère la souris (Claude Code, vim, htop…) : on lui envoie la molette.
            let code = if whole > 0.0 { mouse::WHEEL_UP } else { mouse::WHEEL_DOWN };
            let (column, row) = self.cell_at(event.position);
            if let Some(bytes) = mouse::encode(code, Action::Press, column, row, &event.modifiers, mode) {
                self.terminal.write(bytes.repeat(count));
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !event.modifiers.shift {
            // Écran alternatif sans souris : pas de scrollback, on envoie des flèches (comme xterm).
            let arrow: &[u8] = match (whole > 0.0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.terminal.write(arrow.repeat(count));
        } else {
            self.terminal.scroll(whole as i32);
            cx.notify();
        }
    }

    /// Le programme a demandé les événements souris. Shift les garde pour Grove.
    fn reports_mouse(&self, modifiers: &Modifiers) -> bool {
        self.terminal.mode().intersects(TermMode::MOUSE_MODE) && !modifiers.shift
    }

    /// Cellule sous `position`, bornée à la grille.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let layout = self.layout.get();
        let offset = position - layout.origin;
        let column = (offset.x / layout.cell_width).max(0.0) as usize;
        let row = (offset.y / layout.line_height).max(0.0) as usize;
        (column.min(layout.columns.saturating_sub(1)), row.min(layout.lines.saturating_sub(1)))
    }

    fn report_button(&mut self, button: MouseButton, action: Action, position: Point<Pixels>, modifiers: &Modifiers) {
        let Some(code) = mouse::button_code(button) else { return };
        let (column, row) = self.cell_at(position);
        if let Some(bytes) = mouse::encode(code, action, column, row, modifiers, self.terminal.mode()) {
            self.terminal.write(bytes);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        // Ctrl+clic sur un lien : prioritaire sur le reporting souris du programme.
        if event.button == MouseButton::Left && event.modifiers.control {
            let (column, row) = self.cell_at(event.position);
            if let Some(link) = self.terminal.link_at(row, column) {
                cx.open_url(&link.uri);
                cx.stop_propagation();
                return;
            }
        }
        if self.reports_mouse(&event.modifiers) {
            self.report_button(event.button, Action::Press, event.position, &event.modifiers);
            cx.stop_propagation();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.reports_mouse(&event.modifiers) {
            self.report_button(event.button, Action::Release, event.position, &event.modifiers);
            cx.stop_propagation();
        }
    }

    /// Recalcule le lien survolé : seulement Ctrl enfoncé, comme dans VS Code.
    fn update_hovered_link(&mut self, modifiers: &Modifiers, cx: &mut Context<Self>) {
        let link = match self.mouse_position {
            Some(position) if modifiers.control => {
                let (column, row) = self.cell_at(position);
                self.terminal.link_at(row, column)
            }
            _ => None,
        };
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.mouse_position = Some(event.position);
        self.update_hovered_link(&event.modifiers, cx);
        let mode = self.terminal.mode();
        if !self.reports_mouse(&event.modifiers) {
            return;
        }
        // 1002 : mouvements bouton enfoncé ; 1003 : tous les mouvements.
        let code = match event.pressed_button.and_then(mouse::button_code) {
            Some(code) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => code,
            None if mode.contains(TermMode::MOUSE_MOTION) => mouse::NO_BUTTON,
            _ => return,
        };
        let cell = self.cell_at(event.position);
        if self.last_motion == Some(cell) {
            return;
        }
        self.last_motion = Some(cell);
        if let Some(bytes) =
            mouse::encode(code + mouse::MOTION, Action::Press, cell.0, cell.1, &event.modifiers, mode)
        {
            self.terminal.write(bytes);
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = self.terminal.clone();
        let font = self.font.clone();
        let font_size = self.font_size;
        let focused = self.focus_handle.is_focused(window);
        let background = to_hsla(terminal.palette().background);

        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let line_height = (font_size * LINE_HEIGHT).round();
        let layout = self.layout.clone();
        let metrics = Metrics { cell_width, line_height, font_size };
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        let overlay = Overlay {
            focused,
            link: self.hovered_link.clone(),
            marked_text: self.marked_text.clone(),
            foreground: terminal.palette().foreground,
        };
        let link_hint = self.hovered_link.as_ref().map(|link| SharedString::from(link.uri.clone()));
        let images = self.images.clone();

        let exited = self.exited.map(|code| match code {
            Some(code) => format!("[processus terminé, code {code}]"),
            None => "[processus terminé]".to_owned(),
        });

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.update_hovered_link(&event.modifiers, cx);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.mouse_position = None;
                    this.update_hovered_link(&Modifiers::default(), cx);
                }
            }))
            .when(self.hovered_link.is_some(), |d| d.cursor_pointer())
            .size_full()
            .bg(background)
            .p_1()
            .child(
                canvas(
                    move |bounds, _, _| {
                        let columns = (bounds.size.width / metrics.cell_width).floor().max(1.0);
                        let lines = (bounds.size.height / metrics.line_height).floor().max(1.0);
                        layout.set(GridLayout {
                            origin: bounds.origin,
                            cell_width: metrics.cell_width,
                            line_height: metrics.line_height,
                            columns: columns as usize,
                            lines: lines as usize,
                            cursor: None,
                        });
                        terminal.resize(
                            columns as u16,
                            lines as u16,
                            f32::from(metrics.cell_width) as u16,
                            f32::from(metrics.line_height) as u16,
                        );
                        let snapshot = terminal.snapshot();
                        layout.set(GridLayout { cursor: snapshot.cursor, ..layout.get() });
                        snapshot
                    },
                    move |bounds, snapshot, window, cx| {
                        let images = &mut images.borrow_mut();
                        paint_grid(bounds, &snapshot, &metrics, &font, &overlay, images, window, cx);
                        // Texte composé (accents, méthode de saisie) : reçu par `EntityInputHandler`.
                        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, view), cx);
                    },
                )
                .size_full(),
            )
            .children(link_hint.map(|uri| {
                div()
                    .absolute()
                    .bottom_2()
                    .left_3()
                    .max_w(px(640.0))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(crate::theme::elevated())
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .text_xs()
                    .text_color(crate::theme::text_muted())
                    .truncate()
                    .child(uri)
            }))
            .children(exited.map(|text| {
                div()
                    .absolute()
                    .bottom_2()
                    .right_3()
                    .text_color(Hsla::from(Rgba { r: 0.6, g: 0.65, b: 0.6, a: 1.0 }))
                    .child(SharedString::from(text))
            }))
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
    }

    /// Texte validé par la méthode de saisie : envoyé tel quel au programme.
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.terminal.scroll_to_bottom();
            self.terminal.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    /// Cellule du curseur : la fenêtre de la méthode de saisie s'ouvre à côté.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.get();
        let (row, column) = layout.cursor?;
        Some(Bounds::new(
            point(
                layout.origin.x + layout.cell_width * column as f32,
                layout.origin.y + layout.line_height * row as f32,
            ),
            size(layout.cell_width, layout.line_height),
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Ce qui change le dessin de la grille : focus (curseur), lien survolé, texte en composition.
struct Overlay {
    focused: bool,
    link: Option<Link>,
    marked_text: Option<String>,
    foreground: Rgb,
}

#[derive(Clone, Copy)]
struct Metrics {
    cell_width: Pixels,
    line_height: Pixels,
    font_size: Pixels,
}

#[allow(clippy::too_many_arguments)]
fn paint_grid(
    bounds: Bounds<Pixels>,
    snapshot: &Snapshot,
    metrics: &Metrics,
    font: &Font,
    overlay: &Overlay,
    images: &mut ImageCache,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = bounds.origin;
    let focused = overlay.focused;
    let cell_origin = |row: usize, column: usize| {
        point(
            origin.x + metrics.cell_width * column as f32,
            origin.y + metrics.line_height * row as f32,
        )
    };
    let cell_bounds = |row: usize, column: usize, width: usize| {
        Bounds::new(
            cell_origin(row, column),
            size(metrics.cell_width * width as f32, metrics.line_height),
        )
    };
    // Même cellule, bords arrondis au pixel physique et calculés à partir de
    // l'origine des voisines : deux cellules adjacentes partagent exactement
    // le même bord, sans liseré d'anticrénelage entre elles.
    let scale = window.scale_factor();
    let device_cell = |row: usize, column: usize, width: usize| {
        let to_device = |v: Pixels| (f32::from(v) * scale).round();
        let (start, end) = (cell_origin(row, column), cell_origin(row + 1, column + width));
        let (x, y) = (to_device(start.x), to_device(start.y));
        builtin_glyphs::Rect { x, y, w: to_device(end.x) - x, h: to_device(end.y) - y }
    };
    let from_device = |r: builtin_glyphs::Rect| {
        Bounds::new(point(px(r.x / scale), px(r.y / scale)), size(px(r.w / scale), px(r.h / scale)))
    };

    // Cellules, avec le curseur plein dessiné en inversant les couleurs.
    let block_cursor = focused && snapshot.cursor_shape == CursorShape::Block;
    let mut cells: Vec<RenderCell> = snapshot.cells.clone();
    if let (true, Some((row, column))) = (block_cursor, snapshot.cursor) {
        if let Some(cell) = cells.iter_mut().find(|c| c.row == row && c.column == column) {
            cell.fg = cell.bg;
            cell.bg = snapshot.cursor_color;
        }
    }
    const SELECTION: Rgb = Rgb { r: 0x3a, g: 0x4d, b: 0x44 };
    for cell in cells.iter_mut().filter(|c| c.selected) {
        cell.bg = SELECTION;
    }
    if let Some(link) = &overlay.link {
        for cell in cells.iter_mut().filter(|c| link.contains(c.row, c.column)) {
            cell.flags.insert(CellFlags::UNDERLINE);
        }
    }

    // 1. Fonds : on fusionne les cellules voisines de même couleur.
    for row in cells.chunk_by(|a, b| a.row == b.row) {
        for run in row.chunk_by(|a, b| a.bg == b.bg && b.column == a.column + width(a)) {
            let first = &run[0];
            if first.bg == snapshot.background {
                continue;
            }
            let columns = run.iter().map(width).sum();
            window.paint_quad(fill(from_device(device_cell(first.row, first.column, columns)), to_hsla(first.bg)));
        }
    }

    // 2. Blocs et traits dessinés en rectangles, retirés du texte.
    // Géométrie calculée en pixels physiques, sur la cellule alignée.
    let stroke = (f32::from(metrics.font_size) * scale / 14.0).round().max(1.0);
    for cell in cells.iter_mut() {
        let bounds = device_cell(cell.row, cell.column, width(cell));
        let Some(glyph) = builtin_glyphs::glyph(cell.c, bounds.w, bounds.h, stroke) else {
            continue;
        };
        let color = Hsla { a: glyph.alpha, ..to_hsla(cell.fg) };
        for r in glyph.rects {
            let r = builtin_glyphs::Rect { x: bounds.x + r.x, y: bounds.y + r.y, ..r };
            window.paint_quad(fill(from_device(r), color));
        }
        cell.c = ' ';
    }

    // Images (protocole graphique de kitty), retirées du texte elles aussi.
    images.paint(&mut cells, &snapshot.images, origin, size(metrics.cell_width, metrics.line_height), window);

    // 3. Texte : un lot par suite de cellules de même style. Les caractères larges
    // et les cellules avec caractères combinants forment leur propre lot pour
    // que l'alignement sur la grille (`force_width`) reste juste.
    let same_batch = |a: &RenderCell, b: &RenderCell| {
        a.fg == b.fg
            && style_flags(a.flags) == style_flags(b.flags)
            && b.column == a.column + 1
            && !a.flags.contains(CellFlags::WIDE_CHAR)
            && !b.flags.contains(CellFlags::WIDE_CHAR)
            && a.zerowidth.is_none()
            && b.zerowidth.is_none()
    };
    for row in cells.chunk_by(|a, b| a.row == b.row) {
        for batch in row.chunk_by(same_batch) {
            let first = &batch[0];
            let decorated = first.flags.intersects(CellFlags::ALL_UNDERLINES | CellFlags::STRIKEOUT);
            if !decorated && batch.iter().all(|c| c.c == ' ' || c.c == '\0') {
                continue;
            }
            let mut text = String::with_capacity(batch.len());
            for cell in batch {
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                if let Some(zerowidth) = &cell.zerowidth {
                    text.push_str(zerowidth);
                }
            }
            let color = to_hsla(first.fg);
            let run = TextRun {
                len: text.len(),
                font: Font {
                    weight: if first.flags.contains(CellFlags::BOLD) {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if first.flags.contains(CellFlags::ITALIC) {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                    ..font.clone()
                },
                color,
                background_color: None,
                underline: first.flags.intersects(CellFlags::ALL_UNDERLINES).then_some(
                    UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(color),
                        wavy: first.flags.contains(CellFlags::UNDERCURL),
                    },
                ),
                strikethrough: first
                    .flags
                    .contains(CellFlags::STRIKEOUT)
                    .then_some(StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
            };
            let force_width = (!first.flags.contains(CellFlags::WIDE_CHAR)).then_some(metrics.cell_width);
            let line = window.text_system().shape_line(
                text.into(),
                metrics.font_size,
                &[run],
                force_width,
            );
            let _ = line.paint(cell_origin(first.row, first.column), metrics.line_height, window, cx);
        }
    }

    let Some((row, column)) = snapshot.cursor else {
        return;
    };

    // 4. Texte en composition (après une touche morte ou dans une méthode de saisie),
    // souligné sur fond opaque à la place du curseur.
    if let Some(text) = overlay.marked_text.as_deref() {
        let color = to_hsla(overlay.foreground);
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text.to_owned().into(), metrics.font_size, &[run], None);
        let origin = cell_origin(row, column);
        let width = line.width.max(metrics.cell_width);
        window.paint_quad(fill(Bounds::new(origin, size(width, metrics.line_height)), to_hsla(snapshot.background)));
        let _ = line.paint(origin, metrics.line_height, window, cx);
        return;
    }

    // 5. Curseurs non pleins (barre, soulignement, bloc creux si la vue n'a pas le focus).
    let color = to_hsla(snapshot.cursor_color);
    let cell = cell_bounds(row, column, 1);
    let thickness = px(2.0);
    let shape = match snapshot.cursor_shape {
        CursorShape::Block if !focused => CursorShape::HollowBlock,
        shape => shape,
    };
    match shape {
        CursorShape::Beam => {
            window.paint_quad(fill(Bounds::new(cell.origin, size(thickness, cell.size.height)), color));
        }
        CursorShape::Underline => {
            let y = cell.origin.y + cell.size.height - thickness;
            window.paint_quad(fill(
                Bounds::new(point(cell.origin.x, y), size(cell.size.width, thickness)),
                color,
            ));
        }
        CursorShape::HollowBlock => {
            let px1 = px(1.0);
            let (o, s) = (cell.origin, cell.size);
            for edge in [
                Bounds::new(o, size(s.width, px1)),
                Bounds::new(point(o.x, o.y + s.height - px1), size(s.width, px1)),
                Bounds::new(o, size(px1, s.height)),
                Bounds::new(point(o.x + s.width - px1, o.y), size(px1, s.height)),
            ] {
                window.paint_quad(fill(edge, color));
            }
        }
        CursorShape::Block | CursorShape::Hidden => {}
    }
}

fn width(cell: &RenderCell) -> usize {
    if cell.flags.contains(CellFlags::WIDE_CHAR) { 2 } else { 1 }
}

fn style_flags(flags: CellFlags) -> CellFlags {
    flags & (CellFlags::BOLD | CellFlags::ITALIC | CellFlags::ALL_UNDERLINES | CellFlags::STRIKEOUT)
}

fn to_hsla(c: Rgb) -> Hsla {
    Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: 1.0 }.into()
}
