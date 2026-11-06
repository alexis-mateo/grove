//! Onglets Claude : lancement de `claude`, suivi des sessions via les hooks,
//! notifications, panneau de session et historique (specs §8).

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    Context, FocusHandle, Hsla, KeyDownEvent, SharedString, Task, Window, div, prelude::*, px,
};
use grove_claude::transcripts::{self, SessionSummary};
use grove_claude::{
    ClaudeStatus, EventServer, Message, MessageKind, NativeOptions, PermissionPrompt, RateLimits, Reply, Session,
    Transition,
};
use grove_core::{Paths, TabId, TabKind};
use grove_term::TerminalOptions;

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::usage::{self, Usage};
use crate::marquee::marquee;
use crate::theme;
use crate::ui::{Icon, hint, icon};

pub(crate) struct ClaudeRuntime {
    /// `None` si le socket n'a pas pu être ouvert : les onglets Claude
    /// fonctionnent, mais sans suivi d'état.
    server: Option<EventServer>,
    settings_file: Option<PathBuf>,
    user_statusline: Option<String>,
    pub sessions: HashMap<TabId, Session>,
    pub panel_visible: bool,
    pub history: Option<History>,
    pub usage: Usage,
    notifications: smol::channel::Sender<TabId>,
    _tasks: [Task<()>; 3],
    _usage_api: Option<Task<()>>,
}

/// Sessions passées de l'espace de travail actif.
pub(crate) struct History {
    /// `None` pendant la lecture des transcripts.
    pub sessions: Option<Vec<SessionSummary>>,
    pub focus: FocusHandle,
    _load: Task<()>,
}

impl ClaudeRuntime {
    pub fn start(paths: &Paths, usage_api: bool, window: &mut Window, cx: &mut Context<GroveView>) -> Self {
        let (tx, rx) = smol::channel::unbounded::<(Message, Option<Reply>)>();
        let server = EventServer::start(&grove_claude::socket_path(&paths.data_dir), move |message, reply| {
            let _ = tx.try_send((message, reply));
        })
        .map_err(|err| log::error!("socket des hooks Claude : {err:#}"))
        .ok();

        let settings_file = paths.data_dir.join("claude-settings.json");
        let settings_file = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| grove_claude::write_settings(&settings_file, &exe))
            .map(|()| settings_file)
            .map_err(|err| log::error!("réglages Claude : {err:#}"))
            .ok();

        let messages = cx.spawn_in(window, async move |this, cx| {
            while let Ok((message, reply)) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.on_claude_message(message, reply, window, cx)).is_err() {
                    break;
                }
            }
        });

        // Clic sur une notification : aller à l'onglet.
        let (notifications, clicked) = smol::channel::unbounded::<TabId>();
        let clicks = cx.spawn_in(window, async move |this, cx| {
            while let Ok(tab) = clicked.recv().await {
                let result = this.update_in(cx, |this, window, cx| {
                    if let Some(key) = this.locate_tab(tab) {
                        this.activate_tab(key, tab, window, cx);
                        window.activate_window();
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        });

        // Rafraîchit les comptes à rebours des limites d'usage.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(std::time::Duration::from_secs(30)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.claude.usage.limits.is_some() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        Self {
            server,
            settings_file,
            user_statusline: grove_claude::user_statusline_command(),
            sessions: HashMap::new(),
            panel_visible: true,
            history: None,
            usage: Usage::load(paths.data_dir.join("usage.json")),
            notifications,
            _tasks: [messages, clicks, ticker],
            _usage_api: usage_api.then(|| poll_usage_api(cx)),
        }
    }
}

impl GroveView {
    /// Options du PTY d'un onglet Claude : `claude` lancé par le shell interactif
    /// de l'utilisateur, avec les réglages de Grove en plus des siens.
    pub(crate) fn claude_options(&mut self, tab: TabId, cwd: PathBuf, session_id: Option<&str>) -> TerminalOptions {
        let mut command = self.config.claude.command.clone();
        if let Some(settings) = &self.claude.settings_file {
            command.push_str(&format!(" --settings {}", shell_quote(&settings.to_string_lossy())));
        }
        if let Some(id) = session_id {
            command.push_str(&format!(" --resume {}", shell_quote(id)));
        }
        // Premier message de la session (review d'une PR…).
        if let Some(prompt) = self.pending_prompts.remove(&tab) {
            command.push_str(&format!(" {}", shell_quote(&prompt)));
        }

        let mut options = self.terminal_options(cwd);
        options.args = vec!["-i".into(), "-c".into(), command];
        options.env.insert(grove_claude::ENV_TAB_ID.into(), tab.to_string());
        if let Some(server) = &self.claude.server {
            options.env.insert(grove_claude::ENV_SOCKET.into(), server.path().to_string_lossy().into_owned());
        }
        if let Some(statusline) = &self.claude.user_statusline {
            options.env.insert(grove_claude::ENV_USER_STATUSLINE.into(), statusline.clone());
        }
        self.claude.sessions.insert(tab, Session::default());
        options
    }

    /// Options de `claude -p` pour la vue native d'un onglet (specs §10.1).
    pub(crate) fn native_options(&mut self, tab: TabId, cwd: PathBuf, session_id: Option<&str>) -> NativeOptions {
        let mut env = std::collections::HashMap::new();
        env.insert(grove_claude::ENV_TAB_ID.to_owned(), tab.to_string());
        let mut permission_prompt = None;
        if let Some(server) = &self.claude.server {
            env.insert(grove_claude::ENV_SOCKET.to_owned(), server.path().to_string_lossy().into_owned());
            // Sans serveur MCP, les actions soumises à permission sont refusées.
            if let Ok(exe) = std::env::current_exe() {
                permission_prompt = Some(PermissionPrompt {
                    mcp_config: grove_claude::mcp_config(&exe, server.path(), &tab.to_string()),
                    tool: grove_claude::permission_tool(),
                });
            }
        }
        self.claude.sessions.insert(tab, Session::default());
        NativeOptions {
            shell: self.config.shell(),
            command: self.config.claude.command.clone(),
            cwd: Some(cwd),
            env,
            settings: self.claude.settings_file.clone(),
            resume: session_id.map(str::to_owned),
            permission_mode: None,
            model: None,
            effort: None,
            permission_prompt,
        }
    }

    /// Mémorise l'id de session d'un onglet pour la reprise (il change après `/clear`).
    pub(crate) fn remember_session_id(&mut self, tab: TabId, session_id: Option<String>) {
        let Some(key) = self.locate_tab(tab) else { return };
        if let Some(t) = self.state.workspace_mut(key).and_then(|w| w.tab_mut(tab))
            && let TabKind::Claude { session_id: saved, .. } = &mut t.kind
            && *saved != session_id
        {
            *saved = session_id;
            self.save();
        }
    }

    fn on_claude_message(&mut self, message: Message, reply: Option<Reply>, window: &mut Window, cx: &mut Context<Self>) {
        log::debug!("claude {:?} : {}", message.kind, message.payload);
        let Ok(tab) = message.tab.parse::<TabId>() else { return };
        let Some(key) = self.locate_tab(tab) else { return };
        if message.kind == MessageKind::Permission {
            // Sans vue native, `reply` est détruit : la demande est refusée.
            let (Some(native), Some(reply)) = (self.natives.get(&tab).cloned(), reply) else { return };
            native.update(cx, |native, cx| native.request_permission(&message.payload, reply, cx));
            // Même effet qu'un hook `PermissionRequest` : badge et notification.
            let tool = message.payload.get("tool_name").cloned().unwrap_or_default();
            let hook = Message {
                tab: message.tab.clone(),
                kind: MessageKind::Hook,
                payload: serde_json::json!({ "hook_event_name": "PermissionRequest", "tool_name": tool }),
            };
            let transition = self.claude.sessions.entry(tab).or_default().apply(&hook);
            if let Some(transition) = transition {
                self.notify_transition(key, tab, transition, window, cx);
            }
            cx.notify();
            return;
        }
        if message.kind == MessageKind::Statusline
            && let Some(limits) = RateLimits::from_statusline(&message.payload)
        {
            let threshold = self.claude.usage.update_from_statusline(limits);
            self.alert_usage(threshold);
        }
        let session = self.claude.sessions.entry(tab).or_default();
        let transition = session.apply(&message);
        let session_id = session.session_id.clone();
        self.remember_session_id(tab, session_id);

        if let Some(transition) = transition {
            self.notify_transition(key, tab, transition, window, cx);
        }
        cx.notify();
    }

    /// Notifie le franchissement d'un seuil de la limite de 5 h.
    fn alert_usage(&mut self, threshold: Option<f64>) {
        if let Some(threshold) = threshold
            && self.config.claude.notifications
        {
            let reset = self
                .claude
                .usage
                .limits
                .as_ref()
                .and_then(|l| l.five_hour?.resets_at)
                .map(|at| format!(" · reset dans {}", usage::countdown(at)))
                .unwrap_or_default();
            let body = if threshold >= 100.0 {
                format!("Limite de 5 h atteinte{reset}")
            } else {
                format!("Limite de 5 h à {threshold:.0} %{reset}")
            };
            system_notification("Usage Claude".into(), body, None, self.claude.notifications.clone());
        }
    }

    fn notify_transition(
        &self,
        key: WorkspaceKey,
        tab: TabId,
        transition: Transition,
        window: &Window,
        cx: &Context<Self>,
    ) {
        let visible = window.is_window_active()
            && self.state.active_workspace == Some(key)
            && self.state.workspace(key).and_then(|w| w.active_tab) == Some(tab);
        if visible || !self.config.claude.notifications {
            return;
        }
        let project = self.state.project(key.0).map(|p| p.name.clone()).unwrap_or_default();
        let summary = format!("{} — {project}", self.tab_label(tab, cx));
        let body = match transition {
            Transition::NeedsInput(reason) => format!("Claude {}", reason.describe()),
            Transition::TurnFinished => "Claude a terminé".to_owned(),
        };
        system_notification(summary, body, Some(tab), self.claude.notifications.clone());
    }

    /// Onglets Claude qui attendent une réponse, dans l'ordre de la sidebar.
    fn waiting_tabs(&self) -> Vec<(WorkspaceKey, TabId)> {
        self.state
            .projects
            .iter()
            .flat_map(|p| p.workspaces.iter().map(move |w| (p.id, w)))
            .flat_map(|(p, w)| w.tabs.iter().map(move |t| ((p, w.id), t.id)))
            .filter(|(_, tab)| {
                matches!(self.claude.sessions.get(tab).map(|s| &s.status), Some(ClaudeStatus::NeedsInput { .. }))
            })
            .collect()
    }

    pub(crate) fn next_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = self.waiting_tabs();
        let current = self.active_tab().map(|(_, tab)| tab);
        let next = match waiting.iter().position(|(_, tab)| Some(*tab) == current) {
            Some(i) => waiting.get(i + 1).or(waiting.first()),
            None => waiting.first(),
        };
        if let Some(&(key, tab)) = next {
            self.activate_tab(key, tab, window, cx);
        }
    }

    pub(crate) fn new_claude_tab(&mut self, session_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.state.active_workspace else { return };
        self.leave_settings(cx);
        if let Some(workspace) = self.state.workspace_mut(key) {
            let shared = workspace.kind == grove_core::WorkspaceKind::Direct
                && workspace.tabs.iter().any(|t| matches!(t.kind, TabKind::Claude { .. }));
            if shared {
                self.notice = Some(crate::worktrees::Notice {
                    workspace: key.1,
                    text: "Plusieurs sessions Claude dans le même dossier : elles modifient les mêmes fichiers.".into(),
                    suggest_worktree: true,
                });
            }
            workspace.add_claude_tab(session_id, self.config.claude.view);
            self.claude.history = None;
            self.github.panel = None;
            self.diff = None;
            self.usage_panel = None;
            self.save();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.github.panel = None;
        self.diff = None;
        self.usage_panel = None;
        if self.claude.history.take().is_some() {
            self.focus_active(window, cx);
        } else if let Some(workspace) = self.state.active_workspace.and_then(|key| self.state.workspace(key)) {
            let focus = cx.focus_handle();
            window.focus(&focus);
            // Les transcripts peuvent peser des dizaines de Mo : lecture hors du thread de l'UI.
            let path = workspace.path.clone();
            let sessions = cx.background_executor().spawn(async move { transcripts::list_sessions(&path) });
            let load = cx.spawn(async move |this, cx| {
                let sessions = sessions.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(history) = &mut this.claude.history {
                        history.sessions = Some(sessions);
                        cx.notify();
                    }
                });
            });
            self.claude.history = Some(History { sessions: None, focus, _load: load });
        }
        cx.notify();
    }

    fn history_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.toggle_history(window, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_history(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let history = self.claude.history.as_ref()?;
        let rows: Vec<_> = history
            .sessions
            .iter()
            .flatten()
            .map(|session| {
                let id = session.id.clone();
                // Titre de la session ; à défaut, le premier prompt tient lieu de titre.
                let (title, subtitle) = match (&session.title, &session.first_prompt) {
                    (Some(title), prompt) => (title.clone(), prompt.clone()),
                    (None, Some(prompt)) => (prompt.clone(), None),
                    (None, None) => ("(sans titre)".to_owned(), None),
                };
                let group = SharedString::from(format!("history-row-{id}"));

                div()
                    .id(group.clone())
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2p5()
                    .rounded_lg()
                    .hover(|s| s.bg(theme::hover()))
                    .cursor_pointer()
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| this.new_claude_tab(Some(id.clone()), window, cx)
                    }))
                    .child(
                        icon(Icon::Claude, theme::text_faint()).group_hover(group, |s| s.text_color(theme::accent())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div().flex().text_color(theme::text()).child(marquee(SharedString::from(format!("history-title-{id}")), title)),
                            )
                            .children(subtitle.map(|s| {
                                div()
                                    .flex()
                                    .text_xs()
                                    .text_color(theme::text_faint())
                                    .child(marquee(SharedString::from(format!("history-prompt-{id}")), s))
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap_0p5()
                            .text_xs()
                            .child(div().text_color(theme::text_muted()).child(relative_time(session.modified)))
                            .children(
                                session.cost_usd.map(|cost| div().text_color(theme::text_faint()).child(format!("{cost:.2} $"))),
                            ),
                    )
            })
            .collect();
        let status = match &history.sessions {
            None => Some("Lecture des sessions…"),
            Some(sessions) if sessions.is_empty() => Some("Aucune session."),
            Some(_) => None,
        };

        Some(
            div()
                .id("history-view")
                .track_focus(&history.focus)
                .on_key_down(cx.listener(Self::history_key_down))
                .size_full()
                .flex()
                .justify_center()
                .text_sm()
                .child(
                    div()
                        .w_full()
                        .max_w(px(820.0))
                        .h_full()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .px_6()
                        .pt_6()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .child(icon(Icon::History, theme::text_muted()))
                                .child(
                                    div()
                                        .text_color(theme::text())
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child("Sessions Claude"),
                                )
                                .child(div().text_color(theme::text_faint()).child("de cet espace"))
                                .child(div().flex_1())
                                .child(hint("Échap", "fermer").text_xs().text_color(theme::text_faint())),
                        )
                        .child(div().h(px(1.0)).mx_3().bg(theme::border()))
                        .child(
                            div()
                                .id("history-list")
                                .flex_1()
                                .overflow_y_scroll()
                                .pb_4()
                                .children(status.map(|text| div().px_3().py_2().text_color(theme::text_muted()).child(text)))
                                .children(rows),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Panneau de la session Claude active : modèle, coût, contexte, outil en cours.
    pub(crate) fn render_session_panel(&self, tab: TabId) -> Option<gpui::AnyElement> {
        if !self.claude.panel_visible {
            return None;
        }
        let session = self.claude.sessions.get(&tab)?;
        let m = &session.metrics;
        let (_, color, status) = status_badge(&session.status);

        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .items_baseline()
                .gap_3()
                .child(div().flex_none().text_xs().text_color(theme::text_faint()).child(label))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(theme::text())
                        .child(value),
                )
        };
        let dash = || "—".to_owned();

        let gauge = m.context_percent.map(|percent| {
            let fill = (percent / 100.0).clamp(0.0, 1.0) as f32;
            let color = if percent >= 80.0 { theme::waiting() } else { theme::accent() };
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(div().text_xs().text_color(theme::text_faint()).child("Contexte"))
                        .child(div().text_color(theme::text()).child(format!("{percent:.0} %"))),
                )
                .child(
                    div()
                        .h(px(4.0))
                        .w_full()
                        .rounded_full()
                        .bg(theme::selected())
                        .child(div().h_full().w(gpui::relative(fill)).rounded_full().bg(color)),
                )
                .children(match (m.context_tokens, m.context_size) {
                    (Some(t), Some(s)) => Some(
                        div().text_xs().text_color(theme::text_faint()).child(format!("{}k / {}k tokens", t / 1000, s / 1000)),
                    ),
                    _ => None,
                })
        });

        let section = || div().flex().flex_col().gap_2p5().pt_4().border_t_1().border_color(theme::border());

        Some(
            div()
                .w(px(260.0))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .gap_4()
                .p_4()
                .bg(theme::panel())
                .border_l_1()
                .border_color(theme::border())
                .text_sm()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon(Icon::Claude, color))
                        .child(div().flex_1().min_w_0().text_color(theme::text()).child(status)),
                )
                .child(
                    section()
                        .child(row(
                            "Modèle",
                            match (&m.model, &m.effort) {
                                (Some(model), Some(effort)) => format!("{model} · {effort}"),
                                (Some(model), None) => model.clone(),
                                (None, _) => dash(),
                            },
                        ))
                        .child(row("Coût", m.cost_usd.map(|c| format!("{c:.2} $")).unwrap_or_else(dash)))
                        .when(m.lines_added.is_some() || m.lines_removed.is_some(), |d| {
                            d.child(row(
                                "Lignes",
                                format!("+{} −{}", m.lines_added.unwrap_or(0), m.lines_removed.unwrap_or(0)),
                            ))
                        })
                        .child(row(
                            "Session",
                            session.session_id.as_deref().map(|id| id[..id.len().min(8)].to_owned()).unwrap_or_else(dash),
                        )),
                )
                .children(gauge.map(|gauge| section().child(gauge)))
                .into_any_element(),
        )
    }
}

/// Interroge l'API d'usage au démarrage puis toutes les 3 min (opt-in, specs §8.5).
/// Pas d'appel tant que la statusline fournit des limites récentes : ccstatusline
/// interroge le même endpoint avec le même jeton, et les appels s'additionnent.
impl ClaudeRuntime {
    /// Active ou coupe la lecture de l'usage par l'API (Paramètres), sans redémarrer.
    pub(crate) fn set_usage_api(&mut self, enabled: bool, cx: &mut Context<GroveView>) {
        if enabled != self._usage_api.is_some() {
            self._usage_api = enabled.then(|| poll_usage_api(cx));
        }
    }
}

fn poll_usage_api(cx: &mut Context<GroveView>) -> Task<()> {
    use grove_claude::usage_api::{self, FetchError, POLL_INTERVAL, RATE_LIMIT_BACKOFF};
    cx.spawn(async move |this, cx| {
        let mut refusals = 0;
        loop {
            let Ok(fresh) = this.update(cx, |this, _| this.claude.usage.statusline_fresh(POLL_INTERVAL)) else {
                break;
            };
            let wait = if fresh {
                POLL_INTERVAL
            } else {
                match cx.background_executor().spawn(async { usage_api::fetch() }).await {
                    Ok(limits) => {
                        refusals = 0;
                        let applied = this.update(cx, |this, cx| {
                            let threshold = this.claude.usage.update(limits);
                            this.alert_usage(threshold);
                            cx.notify();
                        });
                        if applied.is_err() {
                            break;
                        }
                        POLL_INTERVAL
                    }
                    Err(FetchError::RateLimited(after)) => {
                        let wait = after.unwrap_or(RATE_LIMIT_BACKOFF[refusals.min(RATE_LIMIT_BACKOFF.len() - 1)]);
                        refusals += 1;
                        log::info!(
                            "API d'usage : {}, nouvel essai dans {} s",
                            FetchError::RateLimited(after),
                            wait.as_secs()
                        );
                        wait
                    }
                    Err(err) => {
                        log::debug!("API d'usage : {err}");
                        POLL_INTERVAL
                    }
                }
            };
            gpui::Timer::after(wait).await;
        }
    })
}

/// Notification système. Un clic sur `tab` y amène (via `clicked`).
fn system_notification(summary: String, body: String, tab: Option<TabId>, clicked: smol::channel::Sender<TabId>) {
    // `wait_for_action` bloque jusqu'à la fermeture de la notification.
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.appname("Grove").summary(&summary).body(&body);
        if tab.is_some() {
            notification.action("default", "Aller à l'onglet");
        }
        match notification.show() {
            Ok(handle) if tab.is_some() => handle.wait_for_action(|action| {
                if action == "default"
                    && let Some(tab) = tab
                {
                    let _ = clicked.try_send(tab);
                }
            }),
            Ok(_) => {}
            Err(err) => log::warn!("notification : {err}"),
        }
    });
}

/// Pastille, couleur et libellé d'un état de session.
pub(crate) fn status_badge(status: &ClaudeStatus) -> (&'static str, Hsla, String) {
    match status {
        ClaudeStatus::Starting => ("○", theme::text_muted(), "démarrage".into()),
        ClaudeStatus::Working { current_tool: Some(tool) } => ("●", theme::accent(), format!("travaille : {tool}")),
        ClaudeStatus::Working { current_tool: None } => ("●", theme::accent(), "travaille".into()),
        ClaudeStatus::NeedsInput { reason } => ("◐", theme::waiting(), reason.describe()),
        ClaudeStatus::Idle => ("○", theme::text_muted(), "terminé".into()),
        ClaudeStatus::Exited => ("○", theme::text_muted(), "fermé".into()),
    }
}

pub(crate) fn relative_time(time: std::time::SystemTime) -> String {
    let Ok(elapsed) = time.elapsed() else { return "à l'instant".into() };
    match elapsed.as_secs() {
        s if s < 60 => "à l'instant".into(),
        s if s < 3600 => format!("il y a {} min", s / 60),
        s if s < 86_400 => format!("il y a {} h", s / 3600),
        s => format!("il y a {} j", s / 86_400),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
