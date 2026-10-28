//! Vue racine : projets, espaces de travail et onglets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::ops::Range;

use gpui::{
    App, Bounds, Context, DragMoveEvent, Entity, EntityInputHandler, FocusHandle, Focusable, KeyDownEvent,
    PathPromptOptions, Pixels, Point, SharedString, Task, UTF16Selection, Window, actions, div, prelude::*, px,
};
use grove_claude::ClaudeStatus;
use grove_core::{ClaudeView, Config, Paths, ProjectId, State, TabId, TabKind, WorkspaceId, WorkspaceKind};
use grove_term::TerminalOptions;
use grove_git::{CheckoutStatus, Head, HeadWatcher, WorktreesWatcher};

use crate::branches::BranchPicker;
use crate::worktrees::{DialogMode, Notice, RemoveDialog, WorktreeDialog};
use crate::claude::ClaudeRuntime;
use crate::github::GitHub;
use crate::native_view::{NativeView, NativeViewEvent};
use crate::profiles::{Offline, ProfileCatalog, ProfilePicker};
use crate::settings::{ConfigWatch, Settings};
use crate::sidebar::{DEFAULT_SIDEBAR_WIDTH, DraggedSidebar};
use crate::terminal_view::{Exited, TerminalView};
use crate::text_field::TextField;
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
        ToggleClaudeView,
        OpenSettings,
        ShowPullRequests,
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
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
        KeyBinding::new("ctrl-shift-m", ToggleClaudeView, None),
        KeyBinding::new("ctrl-shift-c", crate::terminal_view::Copy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", crate::terminal_view::Paste, Some("Terminal")),
        KeyBinding::new("shift-pageup", crate::terminal_view::ScrollPageUp, Some("Terminal")),
        KeyBinding::new("shift-pagedown", crate::terminal_view::ScrollPageDown, Some("Terminal")),
        // Ctrl+, n'envoie rien d'utile aux programmes du terminal (specs §11.3).
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-shift-p", ShowPullRequests, None),
        // D : à droite, S : en dessous.
        KeyBinding::new("ctrl-shift-d", SplitRight, None),
        KeyBinding::new("ctrl-shift-s", SplitDown, None),
        // Sans split, la touche continue jusqu'au programme (voir `focus_neighbor`).
        KeyBinding::new("alt-left", FocusPaneLeft, None),
        KeyBinding::new("alt-right", FocusPaneRight, None),
        KeyBinding::new("alt-up", FocusPaneUp, None),
        KeyBinding::new("alt-down", FocusPaneDown, None),
    ]
}

pub type WorkspaceKey = (ProjectId, WorkspaceId);

/// Dépôt git d'un espace de travail, suivi en direct.
pub(crate) struct GitState {
    pub head: Option<Head>,
    /// Avance/retard et fichiers modifiés, rafraîchis en tâche de fond.
    pub status: Option<CheckoutStatus>,
    /// Synchronisation avec le remote en cours (bouton de la barre d'onglets).
    pub syncing: bool,
    pub sync_task: Option<Task<()>>,
    _watcher: Option<HeadWatcher>,
}

/// Intervalle de rafraîchissement de l'état git (fichiers modifiés, avance/retard).
const GIT_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Onglet en cours de renommage.
pub(crate) struct Rename {
    pub tab: TabId,
    pub field: TextField,
    pub focus: FocusHandle,
}

pub struct GroveView {
    pub(crate) paths: Paths,
    pub(crate) config: Config,
    pub(crate) state: State,
    pub(crate) terminals: HashMap<TabId, Entity<TerminalView>>,
    /// Onglets Claude en vue native.
    pub(crate) natives: HashMap<TabId, Entity<NativeView>>,
    pub(crate) git: HashMap<WorkspaceId, GitState>,
    pub(crate) sidebar_visible: bool,
    pub(crate) sidebar_width: Pixels,
    /// Écriture différée de la largeur pendant un redimensionnement.
    pub(crate) sidebar_save: Option<Task<()>>,
    /// Le clic en cours vient de fermer un panneau flottant (clic à l'extérieur) :
    /// le bouton qui l'ouvre ne doit pas le rouvrir aussitôt.
    pub(crate) popover_dismissed: bool,
    pub(crate) rename: Option<Rename>,
    pub(crate) claude: ClaudeRuntime,
    pub(crate) branch_picker: Option<BranchPicker>,
    pub(crate) profile_picker: Option<ProfilePicker>,
    /// Profils des projets et hôtes de `~/.ssh/config`.
    pub(crate) profiles: ProfileCatalog,
    /// Onglets SSH sans processus : pas encore connectés, ou connexion fermée.
    pub(crate) offline: HashMap<TabId, Offline>,
    /// Page Paramètres ouverte à la place des onglets.
    pub(crate) settings: Option<Settings>,
    pub(crate) config_watch: Option<ConfigWatch>,
    /// Volet actif de chaque onglet partagé ; absent : le volet principal.
    pub(crate) focused_pane: HashMap<TabId, TabId>,
    /// Écriture différée de l'état pendant le déplacement d'un séparateur.
    pub(crate) layout_save: Option<Task<()>>,
    pub(crate) worktree_dialog: Option<WorktreeDialog>,
    pub(crate) remove_dialog: Option<RemoveDialog>,
    pub(crate) notice: Option<Notice>,
    /// Commande à lancer à la place du shell interactif (onglet « setup » d'un worktree).
    pub(crate) pending_commands: HashMap<TabId, String>,
    /// Premier message d'une session Claude pas encore lancée (review d'une PR…).
    pub(crate) pending_prompts: HashMap<TabId, String>,
    pub(crate) github: GitHub,
    /// Espace dont le prochain changement de branche vient de Grove (pas d'avertissement).
    pub(crate) expected_head_change: Option<WorkspaceId>,
    /// Worktrees de chaque projet, pour importer ceux créés hors de Grove.
    pub(crate) worktree_watchers: HashMap<ProjectId, WorktreesWatcher>,
    pub(crate) worktree_events: smol::channel::Sender<ProjectId>,
    focus_handle: FocusHandle,
    git_events: smol::channel::Sender<(WorkspaceId, Head)>,
    _git_tasks: [Task<()>; 3],
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

        // `git worktree add` écrit en plusieurs fois : on attend un peu, puis on importe.
        let (worktree_events, worktree_rx) = smol::channel::unbounded::<ProjectId>();
        let worktree_task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = worktree_rx.recv().await {
                gpui::Timer::after(std::time::Duration::from_millis(500)).await;
                let mut projects = vec![first];
                while let Ok(project) = worktree_rx.try_recv() {
                    if !projects.contains(&project) {
                        projects.push(project);
                    }
                }
                let alive = this.update_in(cx, |this, window, cx| {
                    for project in projects {
                        this.import_worktrees(project, window, cx);
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
            profile_picker: None,
            profiles: ProfileCatalog::default(),
            offline: HashMap::new(),
            settings: None,
            config_watch: None,
            focused_pane: HashMap::new(),
            layout_save: None,
            worktree_dialog: None,
            remove_dialog: None,
            notice: None,
            pending_commands: HashMap::new(),
            pending_prompts: HashMap::new(),
            github: GitHub::default(),
            expected_head_change: None,
            paths,
            config,
            state,
            terminals: HashMap::new(),
            natives: HashMap::new(),
            git: HashMap::new(),
            sidebar_visible: true,
            sidebar_width,
            sidebar_save: None,
            popover_dismissed: false,
            rename: None,
            worktree_watchers: HashMap::new(),
            worktree_events,
            focus_handle: cx.focus_handle(),
            git_events,
            _git_tasks: [git_task, git_poll, worktree_task],
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
        let projects: Vec<_> = this.state.projects.iter().map(|p| (p.id, p.root.clone())).collect();
        for (project, root) in projects {
            this.profiles.reload_project(project, &root);
            this.import_worktrees(project, window, cx);
        }
        this.profiles.reload_ssh();
        this.mark_restored_ssh_tabs();
        this.watch_config(cx);
        this.start_github(cx);

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
        self.git.insert(workspace, GitState { head, status: None, syncing: false, sync_task: None, _watcher: watcher });
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
            self.profiles.reload_project(id, &path);
            self.watch_git(workspace.1, &path, cx);
            self.import_worktrees(id, window, cx);
            self.refresh_github(cx);
        }
        self.activate_workspace(workspace, window, cx);
    }

    pub(crate) fn remove_project(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.state.remove_project(id) else {
            return;
        };
        self.worktree_watchers.remove(&id);
        for workspace in &project.workspaces {
            self.git.remove(&workspace.id);
            for pane in workspace.tabs.iter().flat_map(|t| t.pane_ids()) {
                self.drop_pane(pane);
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
        self.leave_settings(cx);
        self.state.active_workspace = Some(key);
        self.rename = None;
        self.claude.history = None;
        self.github.panel = None;
        self.branch_picker = None;
        self.profile_picker = None;
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
        self.github.panel = None;
        self.leave_settings(cx);
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, key: WorkspaceKey, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.state.workspace_mut(key) else {
            return;
        };
        let panes = workspace.close_tab(tab).map(|t| t.pane_ids()).unwrap_or_else(|| vec![tab]);
        for pane in panes {
            self.drop_pane(pane);
        }
        self.focused_pane.remove(&tab);
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

    pub(crate) fn move_project_to(&mut self, project: ProjectId, index: usize, cx: &mut Context<Self>) {
        if self.state.move_project(project, index) {
            self.save();
            cx.notify();
        }
    }

    pub(crate) fn start_rename(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        // Nom actuel, sélectionné : la saisie le remplace.
        let field = TextField::selected(self.tab_label(tab, cx).to_string());
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.rename = Some(Rename { tab, field, focus });
        cx.notify();
    }

    pub(crate) fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else { return };
        if commit && let Some(key) = self.state.active_workspace {
            if let Some(workspace) = self.state.workspace_mut(key) {
                workspace.rename_tab(rename.tab, rename.field.as_str());
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
        // Onglet d'un profil : son nom, pas le titre du programme (`user@hôte: ~`).
        let profile = match tab_ref.map(|t| &t.kind) {
            Some(TabKind::Terminal { profile: Some(name) }) => Some(name.clone()),
            _ => None,
        };
        let claude = matches!(tab_ref.map(|t| &t.kind), Some(TabKind::Claude { .. }));
        tab_ref
            .and_then(|t| t.title.clone())
            .or(profile)
            .or_else(|| {
                let title = self.terminals.get(&tab)?.read(cx).title()?;
                Some(if claude { strip_status_glyph(title) } else { title }.to_owned())
            })
            .unwrap_or_else(|| fallback.to_owned())
            .into()
    }

    /// Espace de travail qui contient l'onglet `tab`.
    pub(crate) fn locate_tab(&self, tab: TabId) -> Option<WorkspaceKey> {
        self.state.projects.iter().find_map(|p| {
            p.workspaces.iter().find(|w| w.pane_owner(tab).is_some()).map(|w| (p.id, w.id))
        })
    }

    /// Options de base d'un PTY : shell de l'utilisateur dans `cwd`.
    pub(crate) fn terminal_options(&self, cwd: PathBuf) -> TerminalOptions {
        let mut options = TerminalOptions::new(self.config.shell());
        options.cwd = Some(cwd);
        options.scrollback = self.config.terminal.scrollback;
        // Claude Code n'affiche les images (protocole de kitty) que dans kitty et
        // Ghostty, reconnus par leur nom ; Grove les gère aussi, on le lui dit.
        options.env.insert("CLAUDE_CODE_FORCE_TERMINAL_IMAGES".into(), "1".into());
        // De même, il n'émet sa barre de progression (OSC 9;4) que pour ConEmu,
        // Ghostty et iTerm2. Une variable de ConEmu suffit : elle ne change rien
        // d'autre hors de Windows, et `TERM_PROGRAM` reste prioritaire pour nommer le terminal.
        options.env.insert("ConEmuTask".into(), "{Grove}".into());
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
        let kind = workspace.pane_owner(tab)?.pane_kind(tab)?.clone();
        let options = match kind {
            TabKind::Terminal { .. } if self.offline.get(&tab) == Some(&Offline::NotConnected) => return None,
            TabKind::Terminal { profile } => {
                let profile = profile.and_then(|name| {
                    let found = self.find_profile(key.0, &name);
                    if found.is_none() {
                        log::warn!("profil « {name} » introuvable : onglet ouvert avec le shell");
                    }
                    found
                });
                let mut options = match profile {
                    Some(resolved) => self.profile_options(&resolved.profile, &cwd),
                    None => self.terminal_options(cwd),
                };
                if let Some(script) = self.pending_commands.remove(&tab) {
                    options.args = vec!["-c".into(), script];
                }
                options
            }
            TabKind::Claude { view: ClaudeView::Native, .. } => return None,
            TabKind::Claude { session_id, .. } => self.claude_options(tab, cwd, session_id.as_deref()),
        };
        let terminal = match TerminalView::spawn(options, &self.config, cx) {
            Ok(terminal) => terminal,
            Err(err) => {
                log::error!("lancement du terminal : {err:#}");
                return None;
            }
        };
        // Le shell a quitté : on ferme son onglet. Une connexion SSH fermée reste
        // affichée, avec un bandeau pour se reconnecter.
        // Retrouvé par son entité : un volet promu change d'identifiant (`close_pane`).
        cx.subscribe_in(&terminal, window, move |this, emitter, Exited(code): &Exited, window, cx| {
            let Some(pane) = this.terminals.iter().find(|(_, t)| *t == emitter).map(|(id, _)| *id) else { return };
            if this.is_ssh_tab(pane) {
                this.offline.insert(pane, Offline::Closed(*code));
                cx.notify();
            } else {
                this.close_pane(pane, window, cx);
            }
        })
        .detach();
        cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
        self.terminals.insert(tab, terminal.clone());
        Some(terminal)
    }

    /// Vue native d'un onglet Claude, lancée à la première demande.
    fn native(&mut self, key: WorkspaceKey, tab: TabId, cx: &mut Context<Self>) -> Option<Entity<NativeView>> {
        if let Some(native) = self.natives.get(&tab) {
            return Some(native.clone());
        }
        let workspace = self.state.workspace(key)?;
        let cwd = workspace.path.clone();
        let TabKind::Claude { session_id, view: ClaudeView::Native } = workspace.tabs.iter().find(|t| t.id == tab)?.kind.clone()
        else {
            return None;
        };
        let options = self.native_options(tab, cwd, session_id.as_deref());
        let font = crate::fonts::resolve_family(&self.config.font.family);
        let native = cx.new(|cx| NativeView::new(options, font, cx));
        if let Some(prompt) = self.pending_prompts.remove(&tab) {
            native.update(cx, |native, cx| native.submit(&prompt, cx));
        }
        cx.subscribe(&native, move |this, _, event: &NativeViewEvent, cx| {
            match event {
                NativeViewEvent::SessionId(id) => this.remember_session_id(tab, Some(id.clone())),
                NativeViewEvent::PermissionAnswered => {
                    if let Some(session) = this.claude.sessions.get_mut(&tab) {
                        session.status = grove_claude::ClaudeStatus::Working { current_tool: None };
                    }
                }
            }
            cx.notify();
        })
        .detach();
        self.natives.insert(tab, native.clone());
        Some(native)
    }

    /// Vue de l'onglet actif : terminal, ou vue native d'une session Claude.
    fn active_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyView> {
        let (key, _, pane) = self.active_pane()?;
        self.pane_view(key, pane, window, cx)
    }

    /// Vue d'un volet : terminal, ou vue native d'une session Claude.
    pub(crate) fn pane_view(&mut self, key: WorkspaceKey, pane: TabId, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyView> {
        match self.native(key, pane, cx) {
            Some(native) => Some(native.into()),
            None => self.terminal(key, pane, window, cx).map(Into::into),
        }
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Les onglets sont cachés par la page Paramètres : elle garde le focus.
        if let Some(settings) = &self.settings {
            return window.focus(&settings.focus);
        }
        if let Some(panel) = &self.github.panel {
            return window.focus(&panel.focus);
        }
        let focus = self.active_view(window, cx).and_then(|_| {
            let tab = self.active_pane()?.2;
            match self.natives.get(&tab) {
                Some(native) => Some(native.focus_handle(cx)),
                None => Some(self.terminals.get(&tab)?.focus_handle(cx)),
            }
        });
        window.focus(&focus.unwrap_or_else(|| self.focus_handle.clone()));
    }

    /// Bascule l'onglet Claude actif entre le terminal et la vue native (specs §10.4).
    /// Le processus courant s'arrête ; l'autre vue reprend la session (`--resume`).
    pub(crate) fn toggle_claude_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, tab)) = self.active_tab() else { return };
        let Some(t) = self.state.workspace_mut(key).and_then(|w| w.tab_mut(tab)) else { return };
        let TabKind::Claude { view, .. } = &mut t.kind else { return };
        *view = view.toggled();
        // Une seule vue active par session : l'ancienne est détruite avec son processus.
        self.terminals.remove(&tab);
        self.natives.remove(&tab);
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    // Actions clavier.

    /// Nouvel onglet du profil par défaut (`[terminal] default_profile`), sinon le shell.
    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, _)) = self.state.active_workspace else { return };
        let profile = self.config.terminal.default_profile.clone().filter(|name| self.find_profile(project, name).is_some());
        self.open_profile_tab(profile, window, cx);
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // Onglet partagé : seulement le volet actif.
        if let Some((_, _, pane)) = self.active_pane() {
            self.close_pane(pane, window, cx);
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

/// Texte composé par la méthode de saisie, pour le champ qui a le focus
/// (`text_field::input_target`) : traité comme une frappe dans ce champ.
impl EntityInputHandler for GroveView {
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
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let event = KeyDownEvent { keystroke: TextField::composed(text), is_held: false };
        let focused = |focus: &FocusHandle| focus.is_focused(window);
        if self.settings_has_focus(window) {
            self.settings_key_down(&event, window, cx);
        } else if self.rename.as_ref().is_some_and(|r| focused(&r.focus)) {
            self.rename_key_down(&event, window, cx);
        } else if self.branch_picker.as_ref().is_some_and(|p| focused(&p.focus)) {
            self.branch_picker_key_down(&event, window, cx);
        } else if self.profile_picker.as_ref().is_some_and(|p| focused(&p.focus)) {
            self.profile_picker_key_down(&event, window, cx);
        } else if self.worktree_dialog.as_ref().is_some_and(|d| focused(&d.focus)) {
            self.worktree_dialog_key_down(&event, window, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
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
        let panes = active.and_then(|(key, tab)| self.render_panes(key, tab, window, cx));
        let native = active.is_some_and(|(_, tab)| self.natives.contains_key(&tab));

        let project_name = self
            .state
            .active_workspace
            .and_then(|(p, _)| self.state.project(p))
            .map(|p| p.name.clone());
        let title: SharedString = match (active, &project_name) {
            _ if self.settings.is_some() => "Paramètres — Grove".into(),
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
                    .child(match (self.render_pull_requests(cx).or_else(|| self.render_history(cx)), panes) {
                        (Some(view), _) => div().size_full().child(view).into_any_element(),
                        (None, Some(panes)) => div()
                            .size_full()
                            .flex()
                            .child(div().flex_1().min_w_0().h_full().child(panes))
                            // En vue native, le pied de la saisie donne déjà modèle, coût et contexte.
                            .children(active.filter(|_| !native).and_then(|(_, tab)| self.render_session_panel(tab)))
                            .into_any_element(),
                        (None, None) => placeholder(self.state.projects.is_empty()).into_any_element(),
                    })
                    .children(self.render_branch_picker(cx))
                    .children(self.render_profile_picker(cx))
                    .children(self.render_worktree_dialog(cx))
                    .children(self.render_remove_dialog(cx)),
            );

        let content = div()
            .id("grove")
            .key_context("Grove")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Onglet SSH restauré sans connexion : Entrée le connecte.
                if event.keystroke.key == "enter"
                    && this.focus_handle.is_focused(window)
                    && let Some((_, _, pane)) = this.active_pane()
                    && this.offline.get(&pane) == Some(&Offline::NotConnected)
                {
                    this.connect_tab(pane, window, cx);
                    cx.stop_propagation();
                }
            }))
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
            .on_action(cx.listener(|this, _: &ShowPullRequests, window, cx| this.toggle_pull_requests(window, cx)))
            .on_action(cx.listener(|this, _: &OpenBranches, window, cx| this.open_branch_picker(window, cx)))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                if let Some((project, _)) = this.state.active_workspace {
                    this.open_worktree_dialog(project, DialogMode::Create, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleClaudeView, window, cx| this.toggle_claude_view(window, cx)))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| this.split_active(grove_core::Axis::Row, window, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| this.split_active(grove_core::Axis::Column, window, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
                if !this.focus_neighbor(grove_core::Direction::Left, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
                if !this.focus_neighbor(grove_core::Direction::Right, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPaneUp, window, cx| {
                if !this.focus_neighbor(grove_core::Direction::Up, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
                if !this.focus_neighbor(grove_core::Direction::Down, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if this.settings.is_some() {
                    this.close_settings(window, cx);
                } else {
                    this.open_settings(None, None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSessionPanel, _, cx| {
                this.claude.panel_visible = !this.claude.panel_visible;
                cx.notify();
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DraggedSidebar>, _, cx| this.resize_sidebar(event, cx)))
            // Avant les panneaux flottants, qui le relèvent s'ils se ferment sur ce clic.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.popover_dismissed = false))
            .size_full()
            .relative()
            .flex()
            .bg(theme::background())
            .text_color(theme::text())
            .when(self.sidebar_visible, |d| d.child(self.render_sidebar(cx)))
            .child(match self.render_settings(window, cx) {
                Some(settings) => settings,
                None => main.into_any_element(),
            })
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

/// Titre de Claude Code sans son préfixe d'état (`✳ `, spinner `◐ `…) : l'icône de l'onglet
/// le montre déjà, et ce glyphe, pris dans une police de repli, n'est pas aligné sur le texte.
fn strip_status_glyph(title: &str) -> &str {
    let mut chars = title.chars();
    match (chars.next(), chars.next()) {
        (Some(glyph), Some(' ')) if !glyph.is_alphanumeric() && !glyph.is_ascii() => chars.as_str().trim_start(),
        _ => title,
    }
}

#[cfg(test)]
mod tests {
    use super::strip_status_glyph;

    #[test]
    fn strips_claude_status_glyph() {
        assert_eq!(strip_status_glyph("◑ Story kb#12995"), "Story kb#12995");
        assert_eq!(strip_status_glyph("✳ Claude Code"), "Claude Code");
        assert_eq!(strip_status_glyph("Story kb#12995"), "Story kb#12995");
        assert_eq!(strip_status_glyph("é dans le titre"), "é dans le titre");
        assert_eq!(strip_status_glyph("- liste"), "- liste");
    }
}
