//! Vue native d'une session Claude (specs §10) : fil de conversation, saisie,
//! demandes de permission. Pilote `claude -p` en stream-json ; le processus
//! est relancé avec `--resume` quand il le faut (interruption, changement de mode).

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, KeyDownEvent, Pixels, Point, ScrollHandle, SharedString, StyledText,
    Task, UTF16Selection, Window, canvas, div, prelude::*, px,
};
use grove_claude::{Conversation, Entry, NativeEvent, NativeOptions, NativeProcess, Reply, ToolCall};
use serde_json::Value;

use crate::markdown::{self, Block, Inline};
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, hint, icon, icon_button, kbd, tooltip};

/// Modes de permission proposés, dans l'ordre de Maj+Tab (comme le TUI).
/// `bypassPermissions` n'y est pas : trop facile à activer par mégarde.
const MODES: &[(&str, &str)] = &[("default", "Demander"), ("acceptEdits", "Modifications auto"), ("plan", "Plan")];

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
    /// Changement de mode demandé pendant un tour : appliqué à la fin du tour.
    restart_after_turn: bool,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
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
                    let at_bottom = this.at_bottom();
                    for (generation, event) in batch {
                        if generation == this.generation {
                            this.on_process_event(event, cx);
                        }
                    }
                    if at_bottom {
                        this.scroll.scroll_to_bottom();
                    }
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
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
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
        self.scroll.scroll_to_bottom();
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
        self.scroll.scroll_to_bottom();
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
        // Le mode se choisit au lancement : relance au prochain message.
        if self.conversation.busy {
            self.restart_after_turn = true;
        } else if self.process.is_some() {
            self.stop_process();
        }
        cx.notify();
    }

    fn at_bottom(&self) -> bool {
        -self.scroll.offset().y >= self.scroll.max_offset().height - px(32.0)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
            this.scroll.scroll_to_bottom();
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
        let entries: Vec<_> = self
            .conversation
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| self.render_entry(index, entry, cx))
            .collect();
        let empty = entries.is_empty();
        let working = self.conversation.busy && self.permissions.is_empty();

        let thread = div()
            .id("native-thread")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .max_w(px(860.0))
                    .mx_auto()
                    .px_6()
                    .py_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(empty, |d| d.child(empty_state()))
                    .children(entries)
                    .when(working, |d| {
                        d.child(div().text_sm().text_color(theme::text_faint()).child("Claude travaille…  (Échap : interrompre)"))
                    }),
            );

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
            .children(self.render_permission(cx))
            .child(self.render_composer(focused, cx))
    }
}

impl NativeView {
    fn render_entry(&self, index: usize, entry: &Entry, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        Some(match entry {
            Entry::User { text } => div()
                .flex()
                .justify_end()
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
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_color(theme::text())
                    .children(markdown::blocks(text).into_iter().map(|block| self.render_block(block)))
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
                    .flex()
                    .flex_col()
                    .gap_1()
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
                        d.child(div().pl_5().italic().text_color(theme::text_muted()).child(text.clone()))
                    })
                    .into_any_element()
            }
            Entry::Tool(tool) => self.render_tool(tool, cx),
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

    fn render_block(&self, block: Block) -> gpui::AnyElement {
        match block {
            Block::Paragraph(text) => div().child(rich_text(&text)).into_any_element(),
            Block::Heading(text) => {
                div().pt_1().font_weight(FontWeight::SEMIBOLD).child(rich_text(&text)).into_any_element()
            }
            Block::ListItem { marker, depth, text } => div()
                .flex()
                .gap_2()
                .pl(px(16.0 * depth as f32))
                .child(div().flex_none().text_color(theme::text_muted()).child(marker))
                .child(div().flex_1().min_w_0().child(rich_text(&text)))
                .into_any_element(),
            Block::Quote(text) => div()
                .pl_3()
                .border_l_2()
                .border_color(theme::border_strong())
                .text_color(theme::text_muted())
                .child(rich_text(&text))
                .into_any_element(),
            Block::Code(text) => self.code_box(text).into_any_element(),
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
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .when(open, |d| {
                d.child(div().pl_6().flex().flex_col().gap_1().child(self.tool_input(tool)).children(tool.result.as_ref().map(
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
                        self.code_box(text).when(result.is_error, |d| d.text_color(theme::danger()))
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
                .flex()
                .flex_col()
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
                        .flex()
                        .flex_col()
                        .gap_2()
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
                        .child(self.tool_input(&pending.tool))
                        .child(
                            div()
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
        if let Some(model) = &self.conversation.model {
            details.push(model.clone());
        }
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
                        .child(div().flex_1().min_w_0().truncate().child(details.join(" · ")))
                        .child(hint("Maj+Entrée", "nouvelle ligne"))
                        .child(action),
                ),
        )
    }
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

/// Paragraphe avec gras et code en ligne.
fn rich_text(text: &str) -> StyledText {
    let (text, spans) = markdown::inline(text);
    let highlights = spans.into_iter().map(|(range, style)| {
        let style = match style {
            Inline::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
            Inline::Code => HighlightStyle {
                color: Some(theme::accent()),
                background_color: Some(theme::selected()),
                ..Default::default()
            },
        };
        (range, style)
    });
    StyledText::new(text).with_highlights(highlights)
}
