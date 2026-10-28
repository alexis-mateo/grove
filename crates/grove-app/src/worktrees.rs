//! Espaces de travail worktree (specs §9) : création, promotion du travail de
//! l'espace direct, suppression, import des worktrees créés hors de Grove.

use std::path::{Path, PathBuf};

use gpui::{Context, FocusHandle, KeyDownEvent, SharedString, Task, Window, div, prelude::*, px, uniform_list};
use grove_core::{ProjectConfig, ProjectId, WorkspaceId, WorkspaceKind, WorktreeConfig, WorktreeLocation};
use grove_git::{Branch, NewWorktree};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::list::{ListScroll, ROW_HEIGHT};
use crate::marquee::marquee;
use crate::text_field::{self, Edit, TextField};
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, hint, icon, icon_button, list_row, popover, popover_footer, search_field};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogMode {
    /// Nouvel espace sur une branche nouvelle ou existante.
    Create,
    /// Déplace les modifications de l'espace direct dans un nouveau worktree.
    Promote,
}

pub(crate) struct WorktreeDialog {
    project: ProjectId,
    root: PathBuf,
    mode: DialogMode,
    /// Branches du dépôt (`None` pendant le chargement).
    branches: Option<Vec<Branch>>,
    /// Branches déjà extraites dans un worktree : git refuse de les extraire deux fois.
    checked_out: Vec<String>,
    query: TextField,
    selected: usize,
    scroll: ListScroll,
    /// `.grove.toml`, lu à l'ouverture de la fenêtre.
    config: WorktreeConfig,
    busy: bool,
    error: Option<String>,
    pub focus: FocusHandle,
    _task: Option<Task<()>>,
}

#[derive(Clone)]
enum Item {
    New { name: String, base: String },
    Existing(String),
    Remote(String),
}

impl Item {
    fn target(&self) -> NewWorktree {
        match self {
            Item::New { name, base } => NewWorktree::NewBranch { name: name.clone(), base: base.clone() },
            Item::Existing(name) => NewWorktree::Existing(name.clone()),
            Item::Remote(name) => NewWorktree::Remote(name.clone()),
        }
    }
}

/// Suppression d'un worktree en attente de confirmation.
pub(crate) struct RemoveDialog {
    key: WorkspaceKey,
    path: PathBuf,
    branch: Option<String>,
    changed: u32,
    busy: bool,
    error: Option<String>,
    pub focus: FocusHandle,
    _task: Option<Task<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// Retirer de Grove seulement : le dossier reste.
    Forget,
    Delete { branch: bool },
}

/// Avertissement affiché au-dessus du terminal d'un espace (specs §9.2).
pub(crate) struct Notice {
    pub workspace: WorkspaceId,
    pub text: String,
    /// Propose « créer un worktree ».
    pub suggest_worktree: bool,
}

/// Résultat d'une création, appliqué ensuite à l'état de l'UI.
pub(crate) struct Created {
    path: PathBuf,
    branch: String,
    base: String,
    setup: Vec<String>,
}

impl WorktreeDialog {
    /// Bases proposées pour une nouvelle branche : la branche par défaut, puis la branche courante.
    fn bases(&self) -> Vec<String> {
        let Some(branches) = &self.branches else { return Vec::new() };
        let mut bases = Vec::new();
        bases.extend(grove_git::default_branch(branches).map(|b| b.name.clone()));
        if let Some(current) = branches.iter().find(|b| b.current)
            && !bases.contains(&current.name)
        {
            bases.push(current.name.clone());
        }
        bases
    }

    fn items(&self) -> Vec<Item> {
        let name = self.query.as_str().trim();
        if self.mode == DialogMode::Promote {
            // La nouvelle branche part du commit courant, où se trouvent les modifications.
            return if name.is_empty() { Vec::new() } else { vec![Item::New { name: name.into(), base: "HEAD".into() }] };
        }
        let query = name.to_lowercase();
        let branches = self.branches.iter().flatten();
        let exists = branches.clone().any(|b| !b.remote && b.name == name);
        let mut items = Vec::new();
        if !name.is_empty() && !exists {
            items.extend(self.bases().into_iter().map(|base| Item::New { name: name.into(), base }));
        }
        for branch in branches.filter(|b| query.is_empty() || b.name.to_lowercase().contains(&query)) {
            if branch.remote {
                // Inutile de proposer `origin/x` si `x` existe déjà en local.
                let local = branch.name.split_once('/').map_or(branch.name.as_str(), |(_, n)| n);
                if !self.branches.iter().flatten().any(|b| !b.remote && b.name == local) {
                    items.push(Item::Remote(branch.name.clone()));
                }
            } else if !self.checked_out.contains(&branch.name) {
                items.push(Item::Existing(branch.name.clone()));
            }
        }
        items
    }
}

impl GroveView {
    /// Ajoute à la sidebar les worktrees du dépôt créés hors de Grove (specs §9.4).
    pub(crate) fn import_worktrees(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        let Some(root) = self.state.project(project).map(|p| p.root.clone()) else { return };
        if !self.state.project(project).is_some_and(|p| p.workspaces.iter().any(|w| self.git.contains_key(&w.id))) {
            return;
        }
        let list = cx.background_executor().spawn(async move { grove_git::list_worktrees(&root) });
        cx.spawn(async move |this, cx| {
            let Ok(worktrees) = list.await else { return };
            let _ = this.update(cx, |this, cx| {
                let mut added = Vec::new();
                if let Some(p) = this.state.projects.iter_mut().find(|p| p.id == project) {
                    for worktree in worktrees.iter().filter(|w| !w.main && !w.prunable) {
                        let path = worktree.path.canonicalize().unwrap_or_else(|_| worktree.path.clone());
                        if p.workspaces.iter().any(|w| w.path == path) || p.ignored_worktrees.contains(&path) {
                            continue;
                        }
                        let branch = worktree.branch.clone().unwrap_or_else(|| "HEAD détachée".into());
                        added.push((p.add_worktree(path.clone(), &branch, ""), path));
                    }
                }
                if added.is_empty() {
                    return;
                }
                for (id, path) in added {
                    this.watch_git(id, &path, cx);
                }
                this.save();
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_worktree_dialog(
        &mut self,
        project: ProjectId,
        mode: DialogMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.state.project(project).map(|p| p.root.clone()) else { return };
        self.branch_picker = None;
        self.notice = None;
        let focus = cx.focus_handle();
        window.focus(&focus);

        let repo = root.clone();
        let load = cx.background_executor().spawn(async move {
            let branches = grove_git::list_branches(&repo)?;
            let checked_out = grove_git::list_worktrees(&repo)?.into_iter().filter_map(|w| w.branch).collect();
            anyhow::Ok((branches, checked_out))
        });
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = &mut this.worktree_dialog else { return };
                match result {
                    Ok((branches, checked_out)) => {
                        dialog.branches = Some(branches);
                        dialog.checked_out = checked_out;
                    }
                    Err(err) => dialog.error = Some(format!("{err:#}")),
                }
                cx.notify();
            });
        });
        self.worktree_dialog = Some(WorktreeDialog {
            project,
            root: root.clone(),
            mode,
            branches: None,
            checked_out: Vec::new(),
            query: TextField::default(),
            selected: 0,
            scroll: ListScroll::default(),
            config: ProjectConfig::load(&root).unwrap_or_default().worktree,
            busy: false,
            error: None,
            focus,
            _task: Some(task),
        });
        cx.notify();
    }

    fn close_worktree_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.worktree_dialog = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn create_worktree(&mut self, item: Item, with_claude: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.worktree_dialog else { return };
        if dialog.busy {
            return;
        }
        dialog.busy = true;
        dialog.error = None;
        let (project, root, mode) = (dialog.project, dialog.root.clone(), dialog.mode);
        let target = item.target();

        let work = cx.background_executor().spawn(async move { prepare_worktree(&root, &target, mode) });
        dialog._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(created) => {
                    this.worktree_dialog = None;
                    this.add_created_worktree(project, created, with_claude, None, window, cx);
                }
                Err(err) => {
                    if let Some(dialog) = &mut this.worktree_dialog {
                        dialog.busy = false;
                        dialog.error = Some(format!("{err:#}"));
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    /// Ajoute le worktree créé et l'active. `prompt` : premier message de la session Claude.
    pub(crate) fn add_created_worktree(
        &mut self,
        project: ProjectId,
        created: Created,
        with_claude: bool,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.state.projects.iter_mut().find(|p| p.id == project) else { return };
        p.ignored_worktrees.retain(|path| *path != created.path);
        let id = p.add_worktree(created.path.clone(), &created.branch, &created.base);
        let workspace = p.workspace_mut(id).expect("worktree ajouté");
        if !created.setup.is_empty() {
            let tab = workspace.add_terminal_tab();
            workspace.rename_tab(tab, "setup");
            self.pending_commands.insert(tab, setup_script(&created.setup));
        }
        if with_claude {
            let tab = workspace.add_claude_tab(None, self.config.claude.view);
            if let Some(prompt) = prompt {
                self.pending_prompts.insert(tab, prompt);
            }
        }
        self.watch_git(id, &created.path, cx);
        self.activate_workspace((project, id), window, cx);
    }

    pub(crate) fn worktree_dialog_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.worktree_dialog else { return };
        let keystroke = &event.keystroke;
        let count = dialog.items().len();
        match keystroke.key.as_str() {
            "escape" => return self.close_worktree_dialog(window, cx),
            "enter" => {
                if let Some(item) = dialog.items().get(dialog.selected).cloned() {
                    self.create_worktree(item, keystroke.modifiers.shift, window, cx);
                }
            }
            "up" => {
                dialog.selected = dialog.selected.saturating_sub(1);
                dialog.scroll.reveal(dialog.selected, count);
            }
            "down" => {
                dialog.selected = (dialog.selected + 1).min(count.saturating_sub(1));
                dialog.scroll.reveal(dialog.selected, count);
            }
            _ => match dialog.query.key_down(keystroke, cx) {
                Edit::Ignored => return,
                Edit::Changed | Edit::Copy { cut: true, .. } => {
                    dialog.selected = 0;
                    dialog.scroll.reset();
                }
                Edit::Selection | Edit::Copy { .. } => {}
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn render_worktree_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let dialog = self.worktree_dialog.as_ref()?;
        let items = dialog.items();
        let project = self.state.project(dialog.project).map(|p| p.name.clone()).unwrap_or_default();
        let title = match dialog.mode {
            DialogMode::Create => "Nouvel espace de travail",
            DialogMode::Promote => "Déplacer ce travail dans un worktree",
        };
        let placeholder = match dialog.mode {
            DialogMode::Create => "Nom d'une nouvelle branche, ou branche existante…",
            DialogMode::Promote => "Nom de la nouvelle branche…",
        };

        let count = items.len();
        let list = div()
            .relative()
            .h(ListScroll::height(count))
            .child(
                uniform_list(
                    "worktree-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let Some(dialog) = &this.worktree_dialog else { return Vec::new() };
                        let (items, selected) = (dialog.items(), dialog.selected);
                        range
                            .filter_map(|index| Some(this.worktree_row(index, items.get(index)?, index == selected, cx)))
                            .collect()
                    }),
                )
                .track_scroll(dialog.scroll.handle.clone())
                .size_full(),
            )
            .children(dialog.scroll.scrollbar(count));

        let config = &dialog.config;
        let location = match config.location {
            WorktreeLocation::Sibling => format!("../{}.worktrees/", dialog.root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
            WorktreeLocation::Inside => ".worktrees/".to_owned(),
        };
        let status = match (&dialog.branches, dialog.busy) {
            (_, true) => Some("Création du worktree…"),
            (None, _) => Some("Chargement des branches…"),
            _ => None,
        };

        let panel = popover("worktree-dialog", &dialog.focus, px(480.0))
            .child(text_field::input_target(&dialog.focus, cx.entity()))
            .on_key_down(cx.listener(Self::worktree_dialog_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.popover_dismissed = true;
                this.close_worktree_dialog(window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .child(icon(Icon::GitBranch, theme::accent()))
                    .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child(title))
                    .child(div().text_xs().text_color(theme::text_faint()).child(project)),
            )
            .child(search_field(&dialog.query, placeholder))
            .child(div().p_1().child(list))
            .children(status.map(|s| div().px_3().py_2().text_color(theme::text_muted()).child(s)))
            .children(dialog.error.clone().map(|e| div().px_3().py_2().text_color(theme::danger()).child(e)))
            .child(
                popover_footer()
                    .child(hint("Entrée", "terminal"))
                    .child(hint("Maj+Entrée", "session Claude"))
                    .child(div().flex_1())
                    .child(format!(
                        "{location}{}",
                        if config.setup.is_empty() { String::new() } else { format!(" · setup : {} commande(s)", config.setup.len()) }
                    )),
            );
        Some(dialog.scroll.track_drag(panel, count).into_any_element())
    }

    fn worktree_row(&self, index: usize, item: &Item, selected: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let label = match item {
            Item::New { name, base } if base == "HEAD" => format!("Créer « {name} » avec les modifications en cours"),
            Item::New { name, base } => format!("Créer « {name} » depuis « {base} »"),
            Item::Existing(name) => name.clone(),
            Item::Remote(name) => name.clone(),
        };
        let glyph = match item {
            Item::New { .. } => icon(Icon::Plus, theme::accent()),
            Item::Existing(_) => icon(Icon::GitBranch, theme::text_faint()),
            Item::Remote(_) => icon(Icon::ArrowDown, theme::text_faint()),
        };
        list_row(format!("worktree-item-{index}"), selected)
            .h(ROW_HEIGHT)
            .on_click(cx.listener({
                let item = item.clone();
                move |this, event: &gpui::ClickEvent, window, cx| {
                    this.create_worktree(item.clone(), event.modifiers().shift, window, cx)
                }
            }))
            // La couleur va sur la ligne : `marquee` peint avec le style de son parent.
            .text_color(if matches!(item, Item::Remote(_)) { theme::text_muted() } else { theme::text() })
            .child(glyph)
            .child(marquee(SharedString::from(format!("worktree-label-{index}")), label))
            .into_any_element()
    }

    pub(crate) fn open_remove_dialog(&mut self, key: WorkspaceKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.state.workspace(key) else { return };
        let WorkspaceKind::Worktree { branch, .. } = &workspace.kind else { return };
        let branch = self
            .git
            .get(&key.1)
            .and_then(|g| match &g.head {
                Some(grove_git::Head::Branch(name)) => Some(name.clone()),
                _ => None,
            })
            .or_else(|| Some(branch.clone()).filter(|b| !b.is_empty() && b != "HEAD détachée"));
        let changed = self.git.get(&key.1).and_then(|g| g.status).map_or(0, |s| s.changed);
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.remove_dialog = Some(RemoveDialog {
            key,
            path: workspace.path.clone(),
            branch,
            changed,
            busy: false,
            error: None,
            focus,
            _task: None,
        });
        cx.notify();
    }

    fn remove_workspace(&mut self, removal: Removal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.remove_dialog else { return };
        let key = dialog.key;
        if removal == Removal::Forget {
            let path = dialog.path.clone();
            self.remove_dialog = None;
            // Sans quoi l'import automatique le ferait revenir au prochain démarrage.
            if let Some(project) = self.state.projects.iter_mut().find(|p| p.id == key.0)
                && !project.ignored_worktrees.contains(&path)
            {
                project.ignored_worktrees.push(path);
            }
            return self.forget_workspace(key, window, cx);
        }
        let Removal::Delete { branch: delete_branch } = removal else { return };
        let Some(root) = self.state.project(key.0).map(|p| p.root.clone()) else { return };
        let (path, branch, force) = (dialog.path.clone(), dialog.branch.clone(), dialog.changed > 0);
        dialog.busy = true;
        dialog.error = None;

        let work = cx.background_executor().spawn(async move {
            grove_git::remove_worktree(&root, &path, force)?;
            if delete_branch && let Some(branch) = branch {
                // Branche non fusionnée : git refuse, on le signale sans forcer.
                grove_git::delete(&root, &branch, false)
                    .map_err(|err| anyhow::anyhow!("worktree supprimé, branche « {branch} » conservée : {err:#}"))?;
            }
            anyhow::Ok(())
        });
        dialog._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Le worktree a disparu dans tous les cas où l'erreur ne vient que de la branche.
                let worktree_gone = match &result {
                    Ok(()) => true,
                    Err(err) => err.to_string().starts_with("worktree supprimé"),
                };
                if worktree_gone {
                    this.forget_workspace(key, window, cx);
                }
                match result {
                    Ok(()) => this.remove_dialog = None,
                    Err(err) => {
                        if let Some(dialog) = &mut this.remove_dialog {
                            dialog.busy = false;
                            dialog.error = Some(format!("{err:#}"));
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Retire l'espace de Grove (onglets fermés), sans toucher au disque.
    fn forget_workspace(&mut self, key: WorkspaceKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.state.projects.iter_mut().find(|p| p.id == key.0) else { return };
        let Some(workspace) = project.remove_workspace(key.1) else { return };
        let direct = project.workspaces[0].id;
        for tab in &workspace.tabs {
            self.terminals.remove(&tab.id);
            self.claude.sessions.remove(&tab.id);
        }
        self.git.remove(&key.1);
        if self.state.active_workspace == Some(key) {
            self.activate_workspace((key.0, direct), window, cx);
        } else {
            self.save();
            cx.notify();
        }
    }

    fn remove_dialog_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.remove_dialog = None;
            self.focus_active(window, cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_remove_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let dialog = self.remove_dialog.as_ref()?;
        let name = dialog.branch.clone().unwrap_or_else(|| dialog.path.display().to_string());
        let warning = (dialog.changed > 0).then(|| {
            format!("{} fichier(s) modifié(s) ou non suivi(s) seront perdus si tu supprimes le worktree.", dialog.changed)
        });
        let delete_label = if dialog.changed > 0 { "Supprimer quand même" } else { "Supprimer le worktree" };

        Some(
            popover("remove-dialog", &dialog.focus, px(460.0))
                .on_key_down(cx.listener(Self::remove_dialog_key_down))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.popover_dismissed = true;
                    this.remove_dialog = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }))
                .p_4()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_3()
                        .child(
                            div()
                                .size(px(32.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .bg(theme::danger().opacity(0.12))
                                .child(icon(Icon::Trash, theme::danger())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_color(theme::text())
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(format!("Retirer l'espace « {name} » ?")),
                                )
                                .child(div().text_xs().text_color(theme::text_faint()).child(dialog.path.display().to_string())),
                        ),
                )
                .children(warning.map(|w| {
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme::waiting().opacity(0.08))
                        .border_1()
                        .border_color(theme::waiting().opacity(0.25))
                        .text_xs()
                        .text_color(theme::waiting())
                        .child(icon(Icon::Alert, theme::waiting()))
                        .child(div().flex_1().min_w_0().child(w))
                }))
                .children(dialog.error.clone().map(|e| div().text_xs().text_color(theme::danger()).child(e)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .pt_1()
                        .justify_end()
                        .child(button("remove-forget", "Retirer de Grove", ButtonStyle::Secondary).on_click(
                            cx.listener(|this, _, window, cx| this.remove_workspace(Removal::Forget, window, cx)),
                        ))
                        .child(button("remove-delete", delete_label, ButtonStyle::Danger).on_click(cx.listener(
                            |this, _, window, cx| this.remove_workspace(Removal::Delete { branch: false }, window, cx),
                        )))
                        .when(dialog.branch.is_some(), |d| {
                            d.child(
                                button("remove-delete-branch", "Supprimer worktree et branche", ButtonStyle::Danger)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.remove_workspace(Removal::Delete { branch: true }, window, cx)
                                    })),
                            )
                        }),
                )
                .children(dialog.busy.then(|| div().text_xs().text_color(theme::text_muted()).child("git en cours…")))
                .into_any_element(),
        )
    }

    pub(crate) fn render_notice(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let notice = self.notice.as_ref()?;
        let key = self.state.active_workspace.filter(|(_, w)| *w == notice.workspace)?;
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .pl_3()
                .pr_2()
                .py_1p5()
                .bg(theme::waiting().opacity(0.07))
                .border_b_1()
                .border_color(theme::waiting().opacity(0.3))
                .text_sm()
                .child(icon(Icon::Alert, theme::waiting()))
                .child(div().flex_1().min_w_0().text_color(theme::text()).child(notice.text.clone()))
                .when(notice.suggest_worktree, |d| {
                    d.child(button("notice-worktree", "Créer un worktree", ButtonStyle::Primary).on_click(cx.listener(
                        move |this, _, window, cx| this.open_worktree_dialog(key.0, DialogMode::Create, window, cx),
                    )))
                })
                .child(icon_button("notice-close", Icon::X).on_click(cx.listener(|this, _, _, cx| {
                    this.notice = None;
                    cx.notify();
                })))
                .into_any_element(),
        )
    }
}

/// Crée le worktree (hors du thread de l'UI) : dossier, exclusion, promotion, copie des fichiers.
pub(crate) fn prepare_worktree(root: &Path, target: &NewWorktree, mode: DialogMode) -> anyhow::Result<Created> {
    let config = ProjectConfig::load(root)?.worktree;
    let branch = target.branch().to_owned();
    let path = config.path_for(root, &branch);
    if path.exists() {
        anyhow::bail!("{} existe déjà", path.display());
    }
    if config.location == WorktreeLocation::Inside
        && let Some(repo) = grove_git::discover(root)
    {
        grove_git::exclude(&repo.common_dir, "/.worktrees/")?;
    }

    // Promotion : les modifications passent par le stash, partagé entre worktrees.
    let stashed = mode == DialogMode::Promote && grove_git::stash_push(root, &format!("grove : vers {branch}"))?;
    if let Err(err) = grove_git::add_worktree(root, &path, target) {
        if stashed {
            let _ = grove_git::stash_pop(root);
        }
        return Err(err);
    }
    if stashed {
        grove_git::stash_pop(&path).map_err(|err| {
            anyhow::anyhow!("worktree créé, mais les modifications sont restées dans le stash (git stash list) : {err:#}")
        })?;
    }

    for file in &config.copy {
        let (from, to) = (root.join(file), path.join(file));
        if from.exists() {
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::copy(&from, &to).map_err(|err| anyhow::anyhow!("copie de {file} : {err}"))?;
        }
    }

    let base = match target {
        NewWorktree::NewBranch { base, .. } => base.clone(),
        _ => String::new(),
    };
    let path = path.canonicalize().unwrap_or(path);
    Ok(Created { path, branch, base, setup: config.setup_commands(root) })
}

/// Script de l'onglet « setup » : les commandes, leur code de sortie, puis un shell.
fn setup_script(commands: &[String]) -> String {
    let chain = commands.join(" && ");
    format!("{chain}; code=$?; echo; echo \"[setup terminé, code $code]\"; exec \"$SHELL\" -i")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} : {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo(grove_toml: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        run(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join(".grove.toml"), grove_toml).unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "init"]);
        (dir, root)
    }

    #[test]
    fn create_inside_copies_files_and_excludes_folder() {
        let (_dir, root) = repo("[worktree]\nlocation = \"inside\"\ncopy = [\".env\"]\nsetup = [\"echo {{repo}}\"]\n");
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();

        let target = NewWorktree::NewBranch { name: "feat/x".into(), base: "main".into() };
        let created = prepare_worktree(&root, &target, DialogMode::Create).unwrap();
        assert!(created.path.ends_with(".worktrees/feat-x"));
        assert_eq!(created.branch, "feat/x");
        assert_eq!(std::fs::read_to_string(created.path.join(".env")).unwrap(), "SECRET=1");
        assert_eq!(created.setup, ["echo app"]);
        let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/.worktrees/"));
        // Le dossier du worktree n'apparaît pas comme non suivi dans le checkout principal.
        assert_eq!(grove_git::checkout_status(&root).unwrap().changed, 1); // .env seulement

        // Deuxième création sur le même dossier : refusée proprement.
        assert!(prepare_worktree(&root, &target, DialogMode::Create).is_err());
    }

    #[test]
    fn promote_moves_work_out_of_direct() {
        let (dir, root) = repo("");
        std::fs::write(root.join("a.txt"), "en cours").unwrap();
        let target = NewWorktree::NewBranch { name: "wip".into(), base: "HEAD".into() };
        let created = prepare_worktree(&root, &target, DialogMode::Promote).unwrap();
        assert_eq!(created.path, dir.path().join("app.worktrees/wip").canonicalize().unwrap());
        assert_eq!(std::fs::read_to_string(created.path.join("a.txt")).unwrap(), "en cours");
        assert_eq!(grove_git::checkout_status(&root).unwrap().changed, 0);
    }

    #[test]
    fn failed_promotion_restores_changes() {
        let (_dir, root) = repo("");
        std::fs::write(root.join("a.txt"), "en cours").unwrap();
        // Branche existante : `worktree add -b` échoue, les modifications doivent revenir.
        let target = NewWorktree::NewBranch { name: "main".into(), base: "HEAD".into() };
        assert!(prepare_worktree(&root, &target, DialogMode::Promote).is_err());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "en cours");
    }

    #[test]
    fn setup_script_keeps_a_shell() {
        let script = setup_script(&["npm ci".into(), "cp ../app/.env .env".into()]);
        assert!(script.starts_with("npm ci && cp ../app/.env .env; code=$?"));
        assert!(script.ends_with("exec \"$SHELL\" -i"));
    }
}
