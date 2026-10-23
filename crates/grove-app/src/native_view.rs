//! Vue native d'une session Claude (specs §10) : fil de conversation, saisie,
//! demandes de permission. Pilote `claude -p` en stream-json ; le processus
//! est relancé avec `--resume` quand il le faut (interruption, changement de mode).

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, KeyDownEvent, ListAlignment, ListOffset, ListState, Pixels, Point,
    SharedString, StyledText, Task, UTF16Selection, Window, canvas, div, list, prelude::*, px,
};
use grove_claude::{Conversation, Entry, NativeEvent, NativeOptions, NativeProcess, Reply, ToolCall};
use serde_json::Value;

use crate::markdown::{self, Block, Inline};
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, hint, icon, icon_button, kbd, sized_icon, tooltip};

/// Modes de permission proposés, dans l'ordre de Maj+Tab (comme le TUI).
/// `bypassPermissions` n'y est pas : trop facile à activer par mégarde.
const MODES: &[(&str, &str)] = &[("default", "Demander"), ("acceptEdits", "Modifications auto"), ("plan", "Plan")];

/// Modèles proposés : alias de `--model` (`None` : modèle par défaut du compte).
const MODELS: &[(Option<&str>, &str)] = &[
    (None, "Par défaut"),
    (Some("fable"), "Fable"),
    (Some("opus"), "Opus"),
    (Some("sonnet"), "Sonnet"),
    (Some("haiku"), "Haiku"),
];

/// Niveaux de `--effort` (`None` : celui des réglages de Claude Code).
const EFFORTS: &[(Option<&str>, &str)] = &[
    (None, "Par défaut"),
    (Some("low"), "Faible"),
    (Some("medium"), "Moyen"),
    (Some("high"), "Élevé"),
    (Some("xhigh"), "Très élevé"),
    (Some("max"), "Max"),
];

/// Menu ouvert dans le pied du champ de saisie.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Model,
    Effort,
}

/// Hauteur rendue au-delà de la zone visible, pour que le défilement ne
/// découvre pas de messages encore vides.
const OVERDRAW: Pixels = px(800.0);

/// Ce qui, dans une entrée, change son affichage : quand l'empreinte change,
/// la liste remesure l'entrée. Pas de hachage du contenu : la longueur suffit
/// pour le texte qui arrive en morceaux.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    kind: u8,
    len: usize,
    extra: usize,
    open: bool,
}

/// Lignes d'un résultat d'outil affichées une fois déplié.
const RESULT_LINES: usize = 40;

/// Émis vers `GroveView`.
pub enum NativeViewEvent {
    /// Identifiant de session connu ou changé : à mémoriser pour la reprise.
    SessionId(String),
    /// L'utilisateur a répondu à une demande de permission.
    PermissionAnswered,
}

struct PendingPermission {
    tool: ToolCall,
    reply: Reply,
}

pub struct NativeView {
    options: NativeOptions,
    process: Option<NativeProcess>,
    /// Numéro du processus courant : les événements d'un processus remplacé sont ignorés.
    generation: u64,
    events: smol::channel::Sender<(u64, NativeEvent)>,
    conversation: Conversation,
    composer: String,
    /// Texte en cours de composition (touche morte, méthode de saisie).
    marked_text: Option<String>,
    /// Prompts envoyés, pour ↑ / ↓ ; `history_index` : position pendant la navigation.
    history: Vec<String>,
    history_index: Option<usize>,
    permissions: Vec<PendingPermission>,
    /// Outils autorisés pour toute la session (« Toujours »).
    always_allow: HashSet<String>,
    /// Outils dépliés (par id) et réflexions dépliées (par position).
    expanded: HashSet<String>,
    /// Le processus a été interrompu volontairement : sa fin n'est pas une erreur.
    interrupted: bool,
    /// Changement de mode ou de modèle demandé pendant un tour : appliqué à la fin du tour.
    restart_after_turn: bool,
    menu: Option<Menu>,
    /// Menu fermé par l'appui hors de lui : si c'était sur son bouton, le clic
    /// qui suit ne doit pas le rouvrir.
    menu_closed: Option<(Menu, std::time::Instant)>,
    /// Modèle et effort des réglages de Claude Code : affichés tant que la
    /// session n'en a pas annoncé ou qu'aucun n'a été choisi.
    defaults: grove_claude::ConfiguredDefaults,
    focus_handle: FocusHandle,
    /// Fil virtualisé : seuls les messages visibles sont construits et mesurés.
    list: ListState,
    /// Empreintes des entrées au dernier [`Self::sync_list`].
    fingerprints: Vec<Fingerprint>,
    code_font: SharedString,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<NativeViewEvent> for NativeView {}

impl NativeView {
    /// `options.resume` : session à reprendre, dont le fil est relu dans le transcript.
    pub fn new(options: NativeOptions, code_font: String, cx: &mut Context<Self>) -> Self {
        let (events, rx) = smol::channel::unbounded::<(u64, NativeEvent)>();
        let listen = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let mut batch = vec![event];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                }
                let alive = this.update(cx, |this, cx| {
                    // Collée en bas, la liste suit les nouveaux messages d'elle-même.
                    for (generation, event) in batch {
                        if generation == this.generation {
                            this.on_process_event(event, cx);
                        }
                    }
                    this.sync_list();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let mut tasks = vec![listen];
        if let (Some(id), Some(cwd)) = (&options.resume, &options.cwd) {
            let path = grove_claude::transcripts::project_dir(cwd).map(|dir| dir.join(format!("{id}.jsonl")));
            tasks.push(load_history(path, cx));
        }

        let defaults = grove_claude::configured_defaults(options.cwd.as_deref());
        let mut this = Self {
            options,
            process: None,
            generation: 0,
            events,
            conversation: Conversation::default(),
            composer: String::new(),
            marked_text: None,
            history: Vec::new(),
            history_index: None,
            permissions: Vec::new(),
            always_allow: HashSet::new(),
            expanded: HashSet::new(),
            interrupted: false,
            restart_after_turn: false,
            menu: None,
            menu_closed: None,
            defaults,
            focus_handle: cx.focus_handle(),
            list: ListState::new(0, ListAlignment::Bottom, OVERDRAW),
            fingerprints: Vec::new(),
            code_font: code_font.into(),
            _tasks: tasks,
        };
        this.spawn_process();
        this
    }

    fn spawn_process(&mut self) {
        self.generation += 1;
        self.interrupted = false;
        let generation = self.generation;
        let mut options = self.options.clone();
        if let Some(id) = &self.conversation.session_id {
            options.resume = Some(id.clone());
        }
        let tx = self.events.clone();
        match NativeProcess::spawn(&options, move |event| {
            let _ = tx.try_send((generation, event));
        }) {
            Ok(process) => self.process = Some(process),
            Err(err) => self.notice(format!("Lancement de Claude impossible : {err:#}"), true),
        }
    }

    /// Arrête le processus ; le suivant reprendra la session (`--resume`).
    fn stop_process(&mut self) {
        self.generation += 1;
        self.process = None;
        self.conversation.busy = false;
        self.permissions.clear();
    }

    fn notice(&mut self, text: String, error: bool) {
        self.conversation.entries.push(Entry::Notice { text, error });
        self.sync_list();
    }

    fn on_process_event(&mut self, event: NativeEvent, cx: &mut Context<Self>) {
        match event {
            NativeEvent::Line(line) => {
                let session = self.conversation.session_id.clone();
                let was_busy = self.conversation.busy;
                self.conversation.apply_line(&line);
                if let Some(id) = &self.conversation.session_id
                    && session.as_ref() != Some(id)
                {
                    cx.emit(NativeViewEvent::SessionId(id.clone()));
                }
                if was_busy && !self.conversation.busy && std::mem::take(&mut self.restart_after_turn) {
                    self.stop_process();
                }
            }
            NativeEvent::Exited { code, stderr } => {
                self.process = None;
                self.conversation.busy = false;
                self.permissions.clear();
                if std::mem::take(&mut self.interrupted) {
                    self.notice("Interrompu.".into(), false);
                } else if code != Some(0) {
                    let detail = stderr.lines().last().map(|l| format!(" : {l}")).unwrap_or_default();
                    let code = code.map(|c| format!(" (code {c})")).unwrap_or_default();
                    self.notice(format!("Claude s'est arrêté{code}{detail}"), true);
                }
            }
        }
    }

    /// Demande transmise par `grove mcp-permission`.
    pub fn request_permission(&mut self, payload: &Value, reply: Reply, cx: &mut Context<Self>) {
        let tool = ToolCall::from_permission_request(payload);
        if self.always_allow.contains(&tool.name) {
            reply.send(&grove_claude::permission_decision(true, &tool.input));
            cx.emit(NativeViewEvent::PermissionAnswered);
            return;
        }
        self.permissions.push(PendingPermission { tool, reply });
        self.scroll_to_bottom();
        cx.notify();
    }

    fn answer_permission(&mut self, allow: bool, always: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.permissions.is_empty() {
            return;
        }
        let pending = self.permissions.remove(0);
        if always {
            self.always_allow.insert(pending.tool.name.clone());
        }
        // Un refus apparaît dans le fil comme un échec de l'outil.
        pending.reply.send(&grove_claude::permission_decision(allow, &pending.tool.input));
        // Les demandes suivantes pour un outil désormais toujours autorisé.
        if always {
            let (allowed, rest): (Vec<_>, Vec<_>) =
                std::mem::take(&mut self.permissions).into_iter().partition(|p| self.always_allow.contains(&p.tool.name));
            self.permissions = rest;
            for p in allowed {
                p.reply.send(&grove_claude::permission_decision(true, &p.tool.input));
            }
        }
        cx.emit(NativeViewEvent::PermissionAnswered);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.trim().to_owned();
        if text.is_empty() || self.conversation.busy {
            return;
        }
        if self.process.is_none() {
            self.spawn_process();
        }
        let line = self.conversation.user_message(&text);
        if let Some(Err(err)) = self.process.as_ref().map(|p| p.send(&line)) {
            self.notice(format!("Envoi impossible : {err:#}"), true);
            self.conversation.busy = false;
        }
        if self.history.last() != Some(&text) {
            self.history.push(text);
        }
        self.history_index = None;
        self.composer.clear();
        self.sync_list();
        self.scroll_to_bottom();
        cx.notify();
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let (true, Some(process)) = (self.conversation.busy, &self.process) {
            self.interrupted = true;
            process.interrupt();
            cx.notify();
        }
    }

    fn mode(&self) -> &str {
        self.options.permission_mode.as_deref().or(self.conversation.permission_mode.as_deref()).unwrap_or("default")
    }

    fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let index = MODES.iter().position(|(m, _)| *m == self.mode()).map_or(0, |i| (i + 1) % MODES.len());
        self.options.permission_mode = Some(MODES[index].0.to_owned());
        self.restart_with_options(cx);
    }

    fn select(&mut self, menu: Menu, value: Option<&str>, cx: &mut Context<Self>) {
        self.menu = None;
        let option = match menu {
            Menu::Model => &mut self.options.model,
            Menu::Effort => &mut self.options.effort,
        };
        if option.as_deref() != value {
            *option = value.map(str::to_owned);
            self.restart_with_options(cx);
        }
        cx.notify();
    }

    /// Mode et modèle se choisissent au lancement : relance au prochain message,
    /// ou à la fin du tour en cours.
    fn restart_with_options(&mut self, cx: &mut Context<Self>) {
        if self.conversation.busy {
            self.restart_after_turn = true;
        } else if self.process.is_some() {
            self.stop_process();
        }
        cx.notify();
    }

    fn scroll_to_bottom(&self) {
        self.list.scroll_to(ListOffset { item_ix: self.list.item_count(), offset_in_item: px(0.0) });
    }

    fn fingerprint(&self, index: usize, entry: &Entry) -> Fingerprint {
        let print = |kind, len, extra, open| Fingerprint { kind, len, extra, open };
        match entry {
            Entry::User { text } => print(0, text.len(), 0, false),
            Entry::Assistant { text } => print(1, text.len(), 0, false),
            Entry::Thinking { text } => print(2, text.len(), 0, self.expanded.contains(&format!("thinking-{index}"))),
            Entry::Tool(tool) => {
                let input = match &tool.input {
                    Value::Object(map) => map.len(),
                    Value::Null => 0,
                    _ => 1,
                };
                let result = tool.result.as_ref().map_or(0, |r| r.text.len() + 1 + usize::from(r.is_error));
                print(3, tool.summary().len() + input, result, self.expanded.contains(&tool.id))
            }
            Entry::Notice { text, error } => print(4, text.len(), usize::from(*error), false),
            Entry::Summary { text } => print(5, text.len(), 0, self.expanded.contains(&format!("summary-{index}"))),
        }
    }

    /// Signale à la liste les entrées à remesurer : à partir de la première dont
    /// l'empreinte a changé (en pratique, les dernières).
    fn sync_list(&mut self) {
        let prints: Vec<_> =
            self.conversation.entries.iter().enumerate().map(|(i, e)| self.fingerprint(i, e)).collect();
        let old = self.fingerprints.len();
        let first = prints.iter().zip(&self.fingerprints).position(|(a, b)| a != b).unwrap_or(old.min(prints.len()));
        if first < old || first < prints.len() {
            self.list.splice(first..old, prints.len() - first);
        }
        self.fingerprints = prints;
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() && event.keystroke.key == "escape" {
            self.menu = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        let secondary = m.control || m.platform;
        match keystroke.key.as_str() {
            "enter" if m.shift || m.alt => self.composer.push('\n'),
            "enter" if !secondary => self.send(cx),
            "escape" if self.conversation.busy => self.interrupt(cx),
            "tab" if m.shift => self.cycle_mode(cx),
            "backspace" if secondary => delete_word(&mut self.composer),
            "backspace" => {
                self.composer.pop();
            }
            "v" if secondary => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.composer.push_str(&text.replace("\r\n", "\n"));
                }
            }
            "up" if self.composer.is_empty() || self.history_index.is_some() => self.browse_history(-1),
            "down" if self.history_index.is_some() => self.browse_history(1),
            _ if secondary => return,
            _ => match &keystroke.key_char {
                Some(text) if !text.chars().any(char::is_control) => self.composer.push_str(text),
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn browse_history(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() as isize - 1;
        let index = match self.history_index {
            Some(i) => i as isize + delta,
            None => last,
        };
        if index > last {
            self.history_index = None;
            self.composer.clear();
        } else {
            let index = index.max(0) as usize;
            self.history_index = Some(index);
            self.composer = self.history[index].clone();
        }
    }

    fn toggle_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        self.sync_list();
        cx.notify();
    }
}

/// Relit le fil d'une session reprise, hors du thread de l'UI.
fn load_history(path: Option<PathBuf>, cx: &mut Context<NativeView>) -> Task<()> {
    let read = cx.background_executor().spawn(async move {
        let text = std::fs::read_to_string(path?).ok()?;
        Some(Conversation::from_transcript(&text))
    });
    cx.spawn(async move |this, cx| {
        let Some(earlier) = read.await else { return };
        let _ = this.update(cx, |this, cx| {
            this.conversation.prepend_history(earlier);
            // Tout le fil change d'indices : on repart de zéro, collé en bas.
            this.fingerprints.clear();
            this.list.reset(0);
            this.sync_list();
            cx.notify();
        });
    })
}

fn delete_word(text: &mut String) {
    let trimmed = text.trim_end().len();
    let start = text[..trimmed].rfind(char::is_whitespace).map_or(0, |i| i + 1);
    text.truncate(start);
}

impl Focusable for NativeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for NativeView {
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
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.chars().any(char::is_control) {
            self.composer.push_str(text);
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

    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for NativeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.conversation.entries.is_empty();
        let working = self.conversation.busy && self.permissions.is_empty();
        let column = || div().w_full().max_w(px(860.0)).mx_auto().px_6();

        let thread = if empty {
            div().flex_1().min_h_0().child(column().child(empty_state())).into_any_element()
        } else {
            list(
                self.list.clone(),
                cx.processor(move |this, index: usize, _, cx| {
                    let Some(entry) = this.conversation.entries.get(index) else { return div().into_any_element() };
                    // Chaque message est mesuré seul, à la largeur de la liste. Pas de
                    // colonne flex à l'intérieur : avec de longs textes qui passent à la
                    // ligne, elle réserve des hauteurs trop petites (chevauchements).
                    match this.render_entry(index, entry, cx) {
                        // Enveloppe pleine largeur : l'élément racine ne peut pas se centrer seul.
                        Some(element) => div()
                            .w_full()
                            .child(column().when(index == 0, |d| d.pt_4()).pb_3().child(element))
                            .into_any_element(),
                        None => div().into_any_element(),
                    }
                }),
            )
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let working = working.then(|| {
            column()
                .pb_2()
                .text_sm()
                .text_color(theme::text_faint())
                .child("Claude travaille…  (Échap : interrompre)")
        });

        let focused = self.focus_handle.is_focused(window);
        div()
            .id("native-view")
            .key_context("NativeClaude")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_sm()
            .child(thread)
            .children(working)
            .children(self.render_permission(cx))
            .child(self.render_composer(focused, cx))
    }
}

impl NativeView {
    fn render_entry(&self, index: usize, entry: &Entry, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        Some(match entry {
            Entry::User { text } => div()
                .flex()
                .flex_col()
                .items_end()
                .child(
                    div()
                        .max_w(gpui::relative(0.85))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(theme::selected())
                        .text_color(theme::text())
                        .child(text.clone()),
                )
                .into_any_element(),
            Entry::Assistant { text } if text.trim().is_empty() => return None,
            Entry::Assistant { text } => {
                let group = SharedString::from(format!("assistant-{index}"));
                let copied = text.clone();
                div()
                    .group(group.clone())
                    .relative()
                    .text_color(theme::text())
                    .children(
                        markdown::blocks(text)
                            .into_iter()
                            .enumerate()
                            .map(|(i, block)| {
                                div().when(i > 0, |d| d.pt_2()).child(self.render_block(format!("md-{index}-{i}").into(), block))
                            }),
                    )
                    .child(
                        div().absolute().top_0().right_0().invisible().group_hover(group, |s| s.visible()).child(
                            icon_button(format!("copy-{index}"), Icon::Copy)
                                .tooltip(tooltip("Copier la réponse"))
                                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))),
                        ),
                    )
                    .into_any_element()
            }
            Entry::Thinking { text } if text.trim().is_empty() => return None,
            Entry::Thinking { text } => {
                let key = format!("thinking-{index}");
                let open = self.expanded.contains(&key);
                div()
                    .child(
                        div()
                            .id(SharedString::from(key.clone()))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_color(theme::text_faint())
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key.clone(), cx)))
                            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::text_faint()))
                            .child("Réflexion"),
                    )
                    .when(open, |d| {
                        d.child(div().pt_1().pl_5().italic().text_color(theme::text_muted()).child(text.clone()))
                    })
                    .into_any_element()
            }
            Entry::Tool(tool) => self.render_tool(tool, cx),
            Entry::Summary { text } => {
                let key = format!("summary-{index}");
                let open = self.expanded.contains(&key);
                div()
                    .child(
                        div()
                            .id(SharedString::from(key.clone()))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key.clone(), cx)))
                            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::text_faint()))
                            .child("Conversation compactée : la suite repart de ce résumé"),
                    )
                    .when(open, |d| d.child(div().pt_1().child(self.code_box(text.clone()).text_color(theme::text_muted()))))
                    .into_any_element()
            }
            Entry::Notice { text, error } => div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(if *error { theme::danger() } else { theme::text_faint() })
                .child(icon(Icon::Alert, if *error { theme::danger() } else { theme::text_faint() }))
                .child(text.clone())
                .into_any_element(),
        })
    }

    fn render_block(&self, key: SharedString, block: Block) -> gpui::AnyElement {
        match block {
            Block::Paragraph(text) => div().child(rich_text(key, &text)).into_any_element(),
            Block::Heading(level, text) => div()
                .pt_1()
                .font_weight(FontWeight::SEMIBOLD)
                .map(|d| match level {
                    1 => d.pt_2().text_lg(),
                    2 => d.pt_1p5().text_base(),
                    _ => d,
                })
                .child(rich_text(key, &text))
                .into_any_element(),
            Block::ListItem { marker, depth, text } => div()
                .flex()
                .gap_2()
                .pl(px(16.0 * depth as f32))
                .child(div().flex_none().text_color(theme::text_muted()).child(marker))
                .child(div().flex_1().min_w_0().child(rich_text(key, &text)))
                .into_any_element(),
            Block::Quote(text) => div()
                .pl_3()
                .border_l_2()
                .border_color(theme::border_strong())
                .text_color(theme::text_muted())
                .child(rich_text(key, &text))
                .into_any_element(),
            Block::Code(text) => self.code_box(text).into_any_element(),
            Block::Table(table) => render_table(&key, table),
            Block::Rule => div().h(px(1.0)).bg(theme::border()).into_any_element(),
        }
    }

    fn code_box(&self, text: String) -> gpui::Div {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(theme::border())
            .bg(theme::panel())
            .font_family(self.code_font.clone())
            .text_xs()
            .text_color(theme::text())
            .child(text)
    }

    fn render_tool(&self, tool: &ToolCall, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open = self.expanded.contains(&tool.id);
        let (state, color) = match &tool.result {
            None => ("…", theme::text_faint()),
            Some(result) if result.is_error => ("échec", theme::danger()),
            Some(_) => ("", theme::accent()),
        };
        let key = tool.id.clone();
        let header = div()
            .id(SharedString::from(format!("tool-{}", tool.id)))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key.clone(), cx)))
            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::text_faint()))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(tool.name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(self.code_font.clone())
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(tool.summary()),
            )
            .child(match &tool.result {
                Some(result) if !result.is_error => icon(Icon::Check, color).into_any_element(),
                _ => div().flex_none().text_xs().text_color(color).child(state).into_any_element(),
            });

        div()
            .child(header)
            .when(open, |d| {
                d.child(div().pt_1().pl_6().child(self.tool_input(tool)).children(tool.result.as_ref().map(
                    |result| {
                        let mut lines: Vec<&str> = result.text.lines().collect();
                        let hidden = lines.len().saturating_sub(RESULT_LINES);
                        lines.truncate(RESULT_LINES);
                        let mut text = lines.join("\n");
                        if hidden > 0 {
                            text.push_str(&format!("\n… {hidden} lignes de plus"));
                        }
                        if text.is_empty() {
                            text = "(aucune sortie)".into();
                        }
                        div().pt_1().child(self.code_box(text).when(result.is_error, |d| d.text_color(theme::danger())))
                    },
                )))
            })
            .into_any_element()
    }

    /// Entrée d'un outil : différence pour les modifications de fichier, JSON sinon.
    fn tool_input(&self, tool: &ToolCall) -> gpui::AnyElement {
        let field = |key: &str| tool.input.get(key).and_then(Value::as_str);
        if let (Some(old), Some(new)) = (field("old_string"), field("new_string")) {
            let removed = old.lines().map(|l| (format!("- {l}"), theme::danger()));
            let added = new.lines().map(|l| (format!("+ {l}"), theme::accent()));
            return self
                .code_box(String::new())
                .children(removed.chain(added).map(|(line, color)| div().text_color(color).child(line)))
                .into_any_element();
        }
        if let Some(content) = field("content") {
            return self.code_box(content.lines().take(RESULT_LINES).collect::<Vec<_>>().join("\n")).into_any_element();
        }
        let json = serde_json::to_string_pretty(&tool.input).unwrap_or_default();
        self.code_box(json).text_color(theme::text_muted()).into_any_element()
    }

    fn render_permission(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let pending = self.permissions.first()?;
        let name = pending.tool.name.clone();
        let more = self.permissions.len() - 1;
        Some(
            div()
                .mx_auto()
                .w_full()
                .max_w(px(860.0))
                .px_6()
                .pb_2()
                .child(
                    div()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::waiting().opacity(0.5))
                        .bg(theme::waiting().opacity(0.06))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon(Icon::Alert, theme::waiting()))
                                .child(div().text_color(theme::text()).child(format!("Claude veut utiliser {name}")))
                                .when(more > 0, |d| {
                                    d.child(div().text_xs().text_color(theme::text_faint()).child(format!("+ {more} en attente")))
                                }),
                        )
                        .child(div().pt_2().child(self.tool_input(&pending.tool)))
                        .child(
                            div()
                                .pt_2()
                                .flex()
                                .gap_2()
                                .child(button("permission-allow", "Autoriser", ButtonStyle::Primary).on_click(
                                    cx.listener(|this, _, window, cx| this.answer_permission(true, false, window, cx)),
                                ))
                                .child(
                                    button("permission-always", format!("Toujours pour {name}"), ButtonStyle::Secondary)
                                        .tooltip(tooltip("Pour le reste de cette session"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.answer_permission(true, true, window, cx)
                                        })),
                                )
                                .child(button("permission-deny", "Refuser", ButtonStyle::Danger).on_click(cx.listener(
                                    |this, _, window, cx| this.answer_permission(false, false, window, cx),
                                ))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_composer(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.conversation.busy;
        let mode = MODES.iter().find(|(m, _)| *m == self.mode()).map_or("Demander", |(_, label)| label);
        let mut details = Vec::new();
        if let Some(cost) = self.conversation.cost_usd {
            details.push(format!("{cost:.2} $"));
        }
        if let Some(tokens) = self.conversation.context_tokens {
            details.push(format!("{} k tokens de contexte", tokens / 1000));
        }

        let text = div()
            .id("composer-text")
            .relative()
            .min_h(px(40.0))
            .max_h(px(220.0))
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .text_color(theme::text())
            .when(self.composer.is_empty() && self.marked_text.is_none(), |d| {
                d.child(div().text_color(theme::text_faint()).child("Écrire à Claude…"))
            })
            .when(!self.composer.is_empty() || self.marked_text.is_some(), |d| {
                d.child(composer_text(&self.composer, self.marked_text.as_deref(), focused))
            })
            .child({
                // Texte composé (accents, méthode de saisie).
                let view = cx.entity();
                let focus = self.focus_handle.clone();
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx),
                )
                .absolute()
                .size_full()
            });

        let action = if busy {
            button("composer-interrupt", "Interrompre", ButtonStyle::Danger)
                .child(icon(Icon::Square, theme::danger()))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
        } else {
            button("composer-send", "Envoyer", ButtonStyle::Primary)
                .child(icon(Icon::ArrowUp, theme::on_accent()))
                .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
        };

        div().w_full().max_w(px(860.0)).mx_auto().px_6().pb_4().child(
            div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(if focused { theme::border_strong() } else { theme::border() })
                .bg(theme::panel())
                .child(text)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child(
                            div()
                                .id("composer-mode")
                                .px_1p5()
                                .py_0p5()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .text_color(if self.mode() == "default" { theme::text_muted() } else { theme::waiting() })
                                .tooltip(tooltip("Mode de permission (Maj+Tab)"))
                                .on_click(cx.listener(|this, _, _, cx| this.cycle_mode(cx)))
                                .child(mode),
                        )
                        .child(self.render_picker(Menu::Model, cx))
                        .child(self.render_picker(Menu::Effort, cx))
                        .child(div().flex_1().min_w_0().truncate().child(details.join(" · ")))
                        .child(hint("Maj+Entrée", "nouvelle ligne"))
                        .child(action),
                ),
        )
    }
}

impl NativeView {
    /// Modèle affiché : celui annoncé par Claude, sauf s'il vient d'en être choisi
    /// un autre (appliqué au prochain message) ; sinon celui des réglages.
    fn model_label(&self) -> String {
        let name = |alias: &str| MODELS.iter().find(|(a, _)| *a == Some(alias)).map_or(alias.to_owned(), |(_, n)| (*n).to_owned());
        match (&self.options.model, &self.conversation.model) {
            (Some(alias), Some(model)) if model.contains(alias.as_str()) => model_label(model),
            (Some(alias), _) => name(alias),
            (None, Some(model)) => model_label(model),
            (None, None) => self.defaults.model.as_deref().map_or("Modèle par défaut".into(), name),
        }
    }

    /// Effort affiché : choisi, sinon annoncé par la session, sinon celui des réglages.
    fn effort_label(&self) -> String {
        let effort = self.options.effort.as_ref().or(self.conversation.effort.as_ref()).or(self.defaults.effort.as_ref());
        match effort {
            Some(level) => {
                let name = EFFORTS.iter().find(|(l, _)| *l == Some(level.as_str())).map_or(level.as_str(), |(_, n)| n);
                format!("Effort {}", name.to_lowercase())
            }
            None => "Effort par défaut".into(),
        }
    }

    fn render_picker(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, hint, options, current) = match menu {
            Menu::Model => (self.model_label(), "Modèle (appliqué au prochain message)", MODELS, &self.options.model),
            Menu::Effort => (self.effort_label(), "Niveau d'effort (appliqué au prochain message)", EFFORTS, &self.options.effort),
        };
        let open = self.menu == Some(menu);
        let id = match menu {
            Menu::Model => "model",
            Menu::Effort => "effort",
        };
        let chip = div()
            .id(SharedString::from(format!("composer-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .cursor_pointer()
            .text_color(theme::text_muted())
            .hover(|s| s.bg(theme::hover()))
            .when(open, |d| d.bg(theme::selected()))
            .tooltip(tooltip(hint))
            .on_click(cx.listener(move |this, _, _, cx| {
                let just_closed = this.menu_closed.take().is_some_and(|(m, at)| m == menu && at.elapsed().as_millis() < 500);
                this.menu = if this.menu == Some(menu) || just_closed { None } else { Some(menu) };
                cx.notify();
            }))
            .child(label)
            .child(sized_icon(Icon::ChevronDown, px(10.0), theme::text_faint()));

        let items: Vec<_> = options
            .iter()
            .enumerate()
            .map(|(i, &(value, name))| {
                let selected = current.as_deref() == value;
                div()
                    .id((SharedString::from(format!("{id}-option")), i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::text())
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(menu, value, cx)))
                    .child(div().flex_1().child(name))
                    .when(selected, |d| d.child(icon(Icon::Check, theme::accent())))
            })
            .collect();

        div().relative().child(chip).when(open, |d| {
            d.child(gpui::deferred(
                div()
                    .id(SharedString::from(format!("{id}-menu")))
                    .occlude()
                    .absolute()
                    .bottom(px(28.0))
                    .left_0()
                    .w(px(180.0))
                    .p_1()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::elevated())
                    .shadow_lg()
                    .text_sm()
                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.menu_closed = Some((menu, std::time::Instant::now()));
                        cx.notify();
                    }))
                    .children(items),
            ))
        })
    }
}

/// `claude-opus-5-5` → `Opus 5.5`, `…[1m]` → `… 1M` ; un nom inattendu reste tel quel.
fn model_label(model: &str) -> String {
    let (base, window) = match model.split_once('[') {
        Some((base, window)) => (base, format!(" {}", window.trim_end_matches(']').to_uppercase())),
        None => (model, String::new()),
    };
    let Some(rest) = base.strip_prefix("claude-") else { return model.to_owned() };
    let mut parts = rest.split('-');
    let Some(family) = parts.next().filter(|f| f.chars().all(|c| c.is_ascii_alphabetic())) else {
        return model.to_owned();
    };
    // Les dates de version (`20250929`) ne sont pas affichées.
    let version: Vec<_> = parts.take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit())).collect();
    let mut family = family.to_owned();
    if let Some(first) = family.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    if version.is_empty() { family + &window } else { format!("{family} {}{window}", version.join(".")) }
}

fn empty_state() -> impl IntoElement {
    div()
        .pt(px(80.0))
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .text_color(theme::text_muted())
        .child(icon(Icon::Claude, theme::text_faint()))
        .child("Session Claude, vue native")
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(theme::text_faint())
                .child(kbd("Ctrl+Shift+M"))
                .child("bascule vers le terminal (TUI officiel)"),
        )
}

/// Texte saisi, suivi du texte en composition (souligné) et du curseur, dans
/// un seul texte pour que le retour à la ligne automatique les garde ensemble.
fn composer_text(text: &str, marked: Option<&str>, focused: bool) -> StyledText {
    let mut full = text.to_owned();
    let mut highlights = Vec::new();
    if let Some(marked) = marked {
        let start = full.len();
        full.push_str(marked);
        let underline = gpui::UnderlineStyle { thickness: px(1.0), color: Some(theme::text()), wavy: false };
        highlights.push((start..full.len(), HighlightStyle { underline: Some(underline), ..Default::default() }));
    }
    if focused {
        let start = full.len();
        full.push('▏');
        highlights.push((start..full.len(), HighlightStyle { color: Some(theme::accent()), ..Default::default() }));
    }
    StyledText::new(full).with_highlights(highlights)
}

/// Tableau : colonnes proportionnelles à leur contenu le plus long (borné), le
/// texte des cellules passe à la ligne. Les lignes sont empilées en blocs (cf.
/// le fil), chaque ligne est une rangée flex aux largeurs fixées d'avance.
fn render_table(key: &SharedString, table: markdown::Table) -> gpui::AnyElement {
    let columns = table.header.len();
    let weights: Vec<f32> = (0..columns)
        .map(|c| {
            let longest = std::iter::once(&table.header).chain(&table.rows).map(|row| row[c].chars().count()).max();
            longest.unwrap_or(0).clamp(4, 40) as f32
        })
        .collect();
    let row = |r: usize, cells: &[String], header: bool| {
        div()
            .flex()
            .when(r > 0, |d| d.border_t_1().border_color(theme::border()))
            .when(header, |d| d.bg(theme::panel()).font_weight(FontWeight::SEMIBOLD))
            .children(cells.iter().enumerate().map(|(c, cell)| {
                let mut cell_div = div()
                    .flex_basis(px(0.0))
                    .min_w_0()
                    .px_2()
                    .py_1()
                    .when(c > 0, |d| d.border_l_1().border_color(theme::border()))
                    .map(|d| match table.align[c] {
                        markdown::Align::Left => d,
                        markdown::Align::Center => d.text_center(),
                        markdown::Align::Right => d.text_right(),
                    });
                cell_div.style().flex_grow = Some(weights[c]);
                cell_div.child(rich_text(SharedString::from(format!("{key}-{r}-{c}")), cell))
            }))
    };
    div()
        .rounded_md()
        .border_1()
        .border_color(theme::border_strong())
        .child(row(0, &table.header, true))
        .children(table.rows.iter().enumerate().map(|(r, cells)| row(r + 1, cells, false)))
        .into_any_element()
}

/// Paragraphe avec gras, code et liens (ouverts au clic).
fn rich_text(id: impl Into<gpui::ElementId>, text: &str) -> gpui::AnyElement {
    let (text, spans) = markdown::inline(text);
    let mut links = Vec::new();
    let highlights: Vec<_> = spans
        .into_iter()
        .map(|(range, style)| {
            let style = match style {
                Inline::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
                Inline::Italic => HighlightStyle { font_style: Some(gpui::FontStyle::Italic), ..Default::default() },
                Inline::Strike => HighlightStyle {
                    strikethrough: Some(gpui::StrikethroughStyle { thickness: px(1.0), color: None }),
                    color: Some(theme::text_muted()),
                    ..Default::default()
                },
                Inline::Code => HighlightStyle {
                    color: Some(theme::accent()),
                    background_color: Some(theme::selected()),
                    ..Default::default()
                },
                Inline::Link(url) => {
                    links.push((range.clone(), url));
                    HighlightStyle {
                        color: Some(theme::link()),
                        underline: Some(gpui::UnderlineStyle { thickness: px(1.0), color: None, wavy: false }),
                        ..Default::default()
                    }
                }
            };
            (range, style)
        })
        .collect();
    let styled = StyledText::new(text).with_highlights(highlights);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges = links.iter().map(|(range, _)| range.clone()).collect();
    gpui::InteractiveText::new(id, styled)
        .on_click(ranges, move |index, _, cx| cx.open_url(&links[index].1))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_labels() {
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_label("claude-haiku-5-5[1m]"), "Haiku 5.5 1M");
        assert_eq!(model_label("custom-model"), "custom-model");
    }
}
