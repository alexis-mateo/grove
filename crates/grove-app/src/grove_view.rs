//! Vue racine : projets, espaces de travail et onglets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{
    App, Context, DragMoveEvent, Entity, FocusHandle, Focusable, PathPromptOptions, Pixels, SharedString, Task,
    Window, actions, div, prelude::*, px,
};
use grove_claude::ClaudeStatus;
use grove_core::{Config, Paths, ProjectId, State, TabId, TabKind, WorkspaceId, WorkspaceKind};
use grove_term::TerminalOptions;
use grove_git::{CheckoutStatus, Head, HeadWatcher};

use crate::branches::BranchPicker;
use crate::worktrees::{DialogMode, Notice, RemoveDialog, WorktreeDialog};
use crate::claude::ClaudeRuntime;
use crate::sidebar::{DEFAULT_SIDEBAR_WIDTH, DraggedSidebar};
use crate::terminal_view::{Exited, TerminalView};
use crate::ui::{Icon, kbd, sized_icon};
use crate::{theme, window_frame};

actions!(
    grove,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        MoveTabLeft,
        MoveTabRight,
        RenameTab,
        AddProject,
        ToggleSidebar,
        NewClaudeTab,
        NextWaiting,
        ToggleSessionPanel,
        ShowHistory,
        OpenBranches,
        NewWorkspace,
    ]
);

pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    vec![
        KeyBinding::new("ctrl-shift-t", NewTab, None),
        KeyBinding::new("ctrl-shift-w", CloseTab, None),
        KeyBinding::new("ctrl-pagedown", NextTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-pageup", PreviousTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("ctrl-shift-pageup", MoveTabLeft, None),
        KeyBinding::new("ctrl-shift-pagedown", MoveTabRight, None),
        KeyBinding::new("ctrl-shift-r", RenameTab, None),
        KeyBinding::new("ctrl-shift-o", AddProject, None),
        KeyBinding::new("ctrl-shift-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-shift-a", NewClaudeTab, None),
        KeyBinding::new("ctrl-shift-j", NextWaiting, None),
        KeyBinding::new("ctrl-shift-i", ToggleSessionPanel, None),
        KeyBinding::new("ctrl-shift-h", ShowHistory, None),
        KeyBinding::new("ctrl-shift-g", OpenBranches, None),
        KeyBinding::new("ctrl-shift-n", NewWorkspace, None),
    ]
}

pub type WorkspaceKey = (ProjectId, WorkspaceId);

/// Dépôt git d'un espace de travail, suivi en direct.
pub(crate) struct GitState {
    pub head: Option<Head>,
    /// Avance/retard et fichiers modifiés, rafraîchis en tâche de fond.
    pub status: Option<CheckoutStatus>,
    _watcher: Option<HeadWatcher>,
}

/// Intervalle de rafraîchissement de l'état git (fichiers modifiés, avance/retard).
const GIT_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Onglet en cours de renommage.
pub(crate) struct Rename {
    pub tab: TabId,
    pub text: String,
    pub focus: FocusHandle,
}

pub struct GroveView {
    paths: Paths,
    pub(crate) config: Config,
    pub(crate) state: State,
    pub(crate) terminals: HashMap<TabId, Entity<TerminalView>>,
    pub(crate) git: HashMap<WorkspaceId, GitState>,
    pub(crate) sidebar_visible: bool,
    pub(crate) sidebar_width: Pixels,
    /// Écriture différée de la largeur pendant un redimensionnement.
    pub(crate) sidebar_save: Option<Task<()>>,
    pub(crate) rename: Option<Rename>,
    pub(crate) claude: ClaudeRuntime,
    pub(crate) branch_picker: Option<BranchPicker>,
    pub(crate) worktree_dialog: Option<WorktreeDialog>,
    pub(crate) remove_dialog: Option<RemoveDialog>,
    pub(crate) notice: Option<Notice>,
    /// Commande à lancer à la place du shell interactif (onglet « setup » d'un worktree).
    pub(crate) pending_commands: HashMap<TabId, String>,
    /// Espace dont le prochain changement de branche vient de Grove (pas d'avertissement).
    pub(crate) expected_head_change: Option<WorkspaceId>,
    focus_handle: FocusHandle,
    git_events: smol::channel::Sender<(WorkspaceId, Head)>,
    _git_tasks: [Task<()>; 2],
}

impl GroveView {
    pub fn new(
        paths: Paths,
        config: Config,
        open: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = State::load(&paths.state_file()).unwrap_or_else(|err| {
            log::error!("état illisible, on repart de zéro : {err:#}");
            State::default()
        });

        let (git_events, rx) = smol::channel::unbounded::<(WorkspaceId, Head)>();
        let git_task = cx.spawn(async move |this, cx| {
            while let Ok((workspace, head)) = rx.recv().await {
                let updated = this.update(cx, |this, cx| {
                    if let Some(git) = this.git.get_mut(&workspace) {
                        let previous = git.head.replace(head.clone());
                        if previous.is_some_and(|p| p != head) {
                            this.on_external_branch_change(workspace, &head);
                        }
                        this.refresh_git_status(workspace, cx);
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });

        let git_poll = cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(GIT_STATUS_INTERVAL).await;
                let alive = this.update(cx, |this, cx| {
                    let workspaces: Vec<_> = this.git.keys().copied().collect();
                    for workspace in workspaces {
                        this.refresh_git_status(workspace, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let claude = ClaudeRuntime::start(&paths, config.claude.usage_api, window, cx);
        let sidebar_width = state.sidebar_width.map(px).unwrap_or(DEFAULT_SIDEBAR_WIDTH);
        let mut this = Self {
            claude,
            branch_picker: None,
            worktree_dialog: None,
            remove_dialog: None,
            notice: None,
            pending_commands: HashMap::new(),
            expected_head_change: None,
            paths,
            config,
            state,
            terminals: HashMap::new(),
            git: HashMap::new(),
            sidebar_visible: true,
            sidebar_width,
            sidebar_save: None,
            rename: None,
            focus_handle: cx.focus_handle(),
            git_events,
            _git_tasks: [git_task, git_poll],
        };

        let workspaces: Vec<_> = this
            .state
            .projects
            .iter()
            .flat_map(|p| p.workspaces.iter().map(|w| (w.id, w.path.clone())))
            .collect();
        for (id, path) in workspaces {
            this.watch_git(id, &path, cx);
        }
        let projects: Vec<_> = this.state.projects.iter().map(|p| p.id).collect();
        for project in projects {
            this.import_worktrees(project, cx);
        }

        match open {
            Some(path) => this.open_project(&path, window, cx),
            None if this.state.projects.is_empty() => {
                if let Ok(cwd) = std::env::current_dir() {
                    this.open_project(&cwd, window, cx);
                }
            }
            None => {
                if let Some(key) = this.state.active_workspace {
                    this.activate_workspace(key, window, cx);
                }
            }
        }
        this
    }

    pub(crate) fn save(&self) {
        if let Err(err) = self.state.save(&self.paths.state_file()) {
            log::error!("sauvegarde de l'état : {err:#}");
        }
    }

    /// La branche d'un espace direct a changé hors de Grove pendant qu'une session Claude y travaille.
    fn on_external_branch_change(&mut self, workspace: WorkspaceId, head: &Head) {
        if self.expected_head_change.take() == Some(workspace) {
            return;
        }
        let Some(ws) = self.state.projects.iter().flat_map(|p| &p.workspaces).find(|w| w.id == workspace) else {
            return;
        };
        let active = ws.tabs.iter().any(|t| {
            matches!(
                self.claude.sessions.get(&t.id).map(|s| &s.status),
                Some(ClaudeStatus::Working { .. } | ClaudeStatus::NeedsInput { .. })
            )
        });
        if ws.kind == WorkspaceKind::Direct && active {
            self.notice = Some(Notice {
                workspace,
                text: format!("La branche est passée sur « {} » pendant qu'une session Claude travaille ici.", head.label()),
                suggest_worktree: true,
            });
        }
    }

    /// Relit l'état git d'un espace (hors du thread de l'UI).
    pub(crate) fn refresh_git_status(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        let Some(path) = self
            .state
            .projects
            .iter()
            .flat_map(|p| &p.workspaces)
            .find(|w| w.id == workspace)
            .map(|w| w.path.clone())
        else {
            return;
        };
        let status = cx.background_executor().spawn(async move { grove_git::checkout_status(&path) });
        cx.spawn(async move |this, cx| {
            let status = status.await.ok();
            let _ = this.update(cx, |this, cx| {
                if let Some(git) = this.git.get_mut(&workspace)
                    && git.status != status
                {
                    git.status = status;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn watch_git(&mut self, workspace: WorkspaceId, path: &Path, cx: &mut Context<Self>) {
        let Some(repo) = grove_git::discover(path) else {
            return;
        };
        let head = grove_git::read_head(&repo.git_dir).ok();
        let tx = self.git_events.clone();
        let watcher = HeadWatcher::new(&repo.git_dir, move |head| {
            let _ = tx.try_send((workspace, head));
        })
        .map_err(|err| log::warn!("surveillance de {} : {err:#}", repo.git_dir.display()))
        .ok();
        self.git.insert(workspace, GitState { head, status: None, _watcher: watcher });
        self.refresh_git_status(workspace, cx);
    }

    /// Ajoute le projet `path` (ou retrouve celui qui existe) et l'active.
    pub fn open_project(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let existed = self.state.projects.iter().any(|p| p.root == path);
        let id = self.state.add_project(&path);
        let project = self.state.project(id).expect("projet ajouté");
        let workspace = (id, project.workspaces[0].id);
        if !existed {
            self.watch_git(workspace.1, &path, cx);
            self.import_worktrees(id, cx);
        }
        self.activate_workspace(workspace, window, cx);
    }

    pub(crate) fn remove_project(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.state.remove_project(id) else {
            return;
        };
        for workspace in &project.workspaces {
            self.git.remove(&workspace.id);
            for tab in &workspace.tabs {
                self.terminals.remove(&tab.id);
                self.claude.sessions.remove(&tab.id);
            }
        }
        if let Some(key) = self.state.active_workspace {
            self.activate_workspace(key, window, cx);
        } else {
            self.save();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_collapsed(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        if let Some(project) = self.state.projects.iter_mut().find(|p| p.id == id) {
            project.collapsed = !project.collapsed;
            self.save();
            cx.notify();
        }
    }

    /// Active un espace de travail. S'il n'a aucun onglet, on lui en ouvre un.
    pub(crate) fn activate_workspace(&mut self, key: WorkspaceKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.state.workspace_mut(key) else {
            return;
        };
        if workspace.tabs.is_empty() {
            workspace.add_terminal_tab();
        }
        self.state.active_workspace = Some(key);
        self.rename = None;
        self.claude.history = None;
        self.branch_picker = None;
        self.worktree_dialog = None;
        self.remove_dialog = None;
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn activate_tab(&mut self, key: WorkspaceKey, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.state.workspace_mut(key) else {
            return;
        };
        workspace.active_tab = Some(tab);
        self.state.active_workspace = Some(key);
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, key: WorkspaceKey, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.state.workspace_mut(key) else {
            return;
        };
        workspace.close_tab(tab);
        self.terminals.remove(&tab);
        self.claude.sessions.remove(&tab);
        if self.rename.as_ref().is_some_and(|r| r.tab == tab) {
            self.rename = None;
        }
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn move_tab_to(&mut self, tab: TabId, index: usize, cx: &mut Context<Self>) {
        let Some(key) = self.state.active_workspace else { return };
        if let Some(workspace) = self.state.workspace_mut(key) {
            workspace.move_tab(tab, index);
            self.save();
            cx.notify();
        }
    }

    pub(crate) fn start_rename(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.tab_label(tab, cx).to_string();
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.rename = Some(Rename { tab, text, focus });
        cx.notify();
    }

    pub(crate) fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else { return };
        if commit && let Some(key) = self.state.active_workspace {
            if let Some(workspace) = self.state.workspace_mut(key) {
                workspace.rename_tab(rename.tab, &rename.text);
                self.save();
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Titre affiché d'un onglet : nom choisi, sinon titre du programme.
    pub(crate) fn tab_label(&self, tab: TabId, cx: &App) -> SharedString {
        let tab_ref = self
            .state
            .projects
            .iter()
            .flat_map(|p| &p.workspaces)
            .flat_map(|w| &w.tabs)
            .find(|t| t.id == tab);
        let fallback = match tab_ref.map(|t| &t.kind) {
            Some(TabKind::Claude { .. }) => "Claude",
            _ => "Terminal",
        };
        tab_ref
            .and_then(|t| t.title.clone())
            .or_else(|| self.terminals.get(&tab)?.read(cx).title().map(str::to_owned))
            .unwrap_or_else(|| fallback.to_owned())
            .into()
    }

    /// Espace de travail qui contient l'onglet `tab`.
    pub(crate) fn locate_tab(&self, tab: TabId) -> Option<WorkspaceKey> {
        self.state.projects.iter().find_map(|p| {
            p.workspaces.iter().find(|w| w.tabs.iter().any(|t| t.id == tab)).map(|w| (p.id, w.id))
        })
    }

    /// Options de base d'un PTY : shell de l'utilisateur dans `cwd`.
    pub(crate) fn terminal_options(&self, cwd: PathBuf) -> TerminalOptions {
        let mut options = TerminalOptions::new(self.config.shell());
        options.cwd = Some(cwd);
        options.scrollback = self.config.terminal.scrollback;
        options
    }

    pub(crate) fn active_tab(&self) -> Option<(WorkspaceKey, TabId)> {
        let key = self.state.active_workspace?;
        Some((key, self.state.workspace(key)?.active_tab?))
    }

    /// Terminal d'un onglet, lancé à la première demande.
    fn terminal(
        &mut self,
        key: WorkspaceKey,
        tab: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        if let Some(terminal) = self.terminals.get(&tab) {
            return Some(terminal.clone());
        }
        let workspace = self.state.workspace(key)?;
        let cwd = workspace.path.clone();
        let kind = workspace.tabs.iter().find(|t| t.id == tab)?.kind.clone();
        let options = match kind {
            TabKind::Terminal => {
                let mut options = self.terminal_options(cwd);
                if let Some(script) = self.pending_commands.remove(&tab) {
                    options.args = vec!["-c".into(), script];
                }
                options
            }
            TabKind::Claude { session_id } => self.claude_options(tab, cwd, session_id.as_deref()),
        };
        let terminal = match TerminalView::spawn(options, &self.config, cx) {
            Ok(terminal) => terminal,
            Err(err) => {
                log::error!("lancement du terminal : {err:#}");
                return None;
            }
        };
        // Le shell a quitté : on ferme son onglet.
        cx.subscribe_in(&terminal, window, move |this, _, _: &Exited, window, cx| {
            this.close_tab(key, tab, window, cx);
        })
        .detach();
        cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
        self.terminals.insert(tab, terminal.clone());
        Some(terminal)
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab().and_then(|(key, tab)| self.terminal(key, tab, window, cx)) {
            Some(terminal) => window.focus(&terminal.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
    }

    // Actions clavier.

    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.state.active_workspace else { return };
        if let Some(workspace) = self.state.workspace_mut(key) {
            workspace.add_terminal_tab();
            self.save();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((key, tab)) = self.active_tab() {
            self.close_tab(key, tab, window, cx);
        }
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, tab)) = self.active_tab() else { return };
        let tabs = &self.state.workspace(key).expect("espace actif").tabs;
        let index = tabs.iter().position(|t| t.id == tab).unwrap_or(0) as isize;
        let next = tabs[(index + delta).rem_euclid(tabs.len() as isize) as usize].id;
        self.activate_tab(key, next, window, cx);
    }

    fn move_active_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((key, tab)) = self.active_tab() else { return };
        let tabs = &self.state.workspace(key).expect("espace actif").tabs;
        let index = tabs.iter().position(|t| t.id == tab).unwrap_or(0) as isize;
        self.move_tab_to(tab, (index + delta).max(0) as usize, cx);
    }

    pub(crate) fn add_project(&mut self, _: &AddProject, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Ajouter le projet".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| this.open_project(&path, window, cx));
        })
        .detach();
    }

    pub(crate) fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }
}

impl Focusable for GroveView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GroveView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab();
        let terminal = active.and_then(|(key, tab)| self.terminal(key, tab, window, cx));

        let project_name = self
            .state
            .active_workspace
            .and_then(|(p, _)| self.state.project(p))
            .map(|p| p.name.clone());
        let title: SharedString = match (active, &project_name) {
            (Some((_, tab)), Some(project)) => {
                format!("{} — {project} — Grove", self.tab_label(tab, cx)).into()
            }
            (None, Some(project)) => format!("{project} — Grove").into(),
            _ => "Grove".into(),
        };
        window.set_window_title(&title);

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_tab_bar(window, cx))
            .children(self.render_notice(cx))
            .child(
                // Zone sous la barre d'onglets ; le sélecteur de branches s'affiche par-dessus.
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .child(match (self.render_history(cx), terminal) {
                        (Some(history), _) => div().size_full().child(history).into_any_element(),
                        (None, Some(terminal)) => div()
                            .size_full()
                            .flex()
                            .child(
                                // Vue mise en cache : elle n'est redessinée que si le terminal
                                // change, pas à chaque survol ou défilement ailleurs dans la fenêtre.
                                div().flex_1().min_w_0().h_full().child(
                                    gpui::AnyView::from(terminal).cached(gpui::StyleRefinement::default().size_full()),
                                ),
                            )
                            .children(active.and_then(|(_, tab)| self.render_session_panel(tab)))
                            .into_any_element(),
                        (None, None) => placeholder(self.state.projects.is_empty()).into_any_element(),
                    })
                    .children(self.render_branch_picker(cx))
                    .children(self.render_worktree_dialog(cx))
                    .children(self.render_remove_dialog(cx)),
            );

        let content = div()
            .id("grove")
            .key_context("Grove")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &MoveTabLeft, _, cx| this.move_active_tab(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveTabRight, _, cx| this.move_active_tab(1, cx)))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                if let Some((_, tab)) = this.active_tab() {
                    this.start_rename(tab, window, cx);
                }
            }))
            .on_action(cx.listener(Self::add_project))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &NewClaudeTab, window, cx| this.new_claude_tab(None, window, cx)))
            .on_action(cx.listener(|this, _: &NextWaiting, window, cx| this.next_waiting(window, cx)))
            .on_action(cx.listener(|this, _: &ShowHistory, window, cx| this.toggle_history(window, cx)))
            .on_action(cx.listener(|this, _: &OpenBranches, window, cx| this.open_branch_picker(window, cx)))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                if let Some((project, _)) = this.state.active_workspace {
                    this.open_worktree_dialog(project, DialogMode::Create, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSessionPanel, _, cx| {
                this.claude.panel_visible = !this.claude.panel_visible;
                cx.notify();
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DraggedSidebar>, _, cx| this.resize_sidebar(event, cx)))
            .size_full()
            .relative()
            .flex()
            .bg(theme::background())
            .text_color(theme::text())
            .when(self.sidebar_visible, |d| d.child(self.render_sidebar(cx)))
            .child(main)
            .when(self.sidebar_visible, |d| d.child(self.render_sidebar_handle(cx)));

        window_frame::frame(content.into_any_element(), window)
    }
}

fn placeholder(no_project: bool) -> impl IntoElement {
    let (glyph, message, hint, shortcut) = if no_project {
        (Icon::FolderPlus, "Aucun projet", "Ajoute un dossier pour commencer", "Ctrl+Shift+O")
    } else {
        (Icon::Terminal, "Aucun terminal", "Ouvre un terminal dans cet espace", "Ctrl+Shift+T")
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .size(px(44.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::panel())
                .child(sized_icon(glyph, px(20.0), theme::text_muted())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child(message))
                .child(div().text_sm().text_color(theme::text_muted()).child(hint)),
        )
        .child(kbd(shortcut))
}
