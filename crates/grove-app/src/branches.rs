//! Branches en mode direct : pastille de la branche courante et sélecteur
//! (recherche, switch, création, suppression), specs §7.4.

use std::path::PathBuf;

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, FocusHandle, KeyDownEvent, SharedString, Task, Transformation, Window, div,
    percentage, prelude::*, px, uniform_list,
};
use grove_claude::ClaudeStatus;
use grove_core::WorkspaceId;
use grove_git::{Branch, CheckoutStatus};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::list::{ListScroll, ROW_HEIGHT};
use crate::marquee::marquee;
use crate::text_field::{self, Edit, TextField};
use crate::theme;
use crate::ui::{
    ButtonStyle, Icon, button, hint, icon, icon_button, list_row, popover, popover_footer, search_field, sized_icon,
    tooltip,
};
use crate::worktrees::Notice;

pub(crate) struct BranchPicker {
    workspace: WorkspaceKey,
    workdir: PathBuf,
    /// `None` pendant le chargement.
    branches: Option<Vec<Branch>>,
    query: TextField,
    selected: usize,
    scroll: ListScroll,
    confirm: Option<Confirm>,
    error: Option<String>,
    busy: bool,
    pub focus: FocusHandle,
    _task: Option<Task<()>>,
}

/// Action en attente de confirmation.
#[derive(Clone)]
enum Confirm {
    Switch { branch: Branch, warnings: Vec<String> },
    Delete { name: String, force: bool },
}

#[derive(Clone)]
enum Item {
    Branch(Branch),
    /// Nouvelle branche, depuis `base` (la branche courante si `None`).
    Create { name: String, base: Option<String> },
}

enum Op {
    Switch(Branch),
    Create { name: String, base: Option<String> },
    Delete { name: String, force: bool },
}

impl BranchPicker {
    fn items(&self) -> Vec<Item> {
        let name = self.query.as_str().trim();
        let query = name.to_lowercase();
        let branches = self.branches.iter().flatten();
        let mut items: Vec<Item> = branches
            .clone()
            .filter(|b| query.is_empty() || b.name.to_lowercase().contains(&query))
            .cloned()
            .map(Item::Branch)
            .collect();
        let exists = branches.clone().any(|b| !b.remote && b.name == name);
        if !query.is_empty() && !exists {
            items.push(Item::Create { name: name.to_owned(), base: None });
            // Aussi depuis la branche principale, si on n'est pas déjà dessus.
            let current = branches.clone().find(|b| b.current).map(|b| b.name.as_str());
            let default = grove_git::default_branch(self.branches.as_deref().unwrap_or_default());
            if let Some(default) = default.filter(|d| Some(d.name.as_str()) != current) {
                items.push(Item::Create { name: name.to_owned(), base: Some(default.name.clone()) });
            }
        }
        items
    }
}

/// « ↑2 ↓1 ●3 » : avance, retard, fichiers modifiés. Avec un remote, avance et
/// retard s'affichent sur le bouton de synchronisation.
fn status_text(status: &CheckoutStatus) -> String {
    let mut parts = Vec::new();
    if status.has_remote {
        // Rien : voir `render_sync_button`.
    } else if status.ahead > 0 {
        parts.push(format!("↑{}", status.ahead));
    }
    if status.behind > 0 && !status.has_remote {
        parts.push(format!("↓{}", status.behind));
    }
    if status.changed > 0 {
        parts.push(format!("●{}", status.changed));
    }
    parts.join(" ")
}

impl GroveView {
    pub(crate) fn open_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.branch_picker.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let Some(key) = self.state.active_workspace else { return };
        if !self.git.contains_key(&key.1) {
            return;
        }
        let Some(workdir) = self.state.workspace(key).map(|w| w.path.clone()) else { return };
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.branch_picker = Some(BranchPicker {
            workspace: key,
            workdir,
            branches: None,
            query: TextField::default(),
            selected: 0,
            scroll: ListScroll::default(),
            confirm: None,
            error: None,
            busy: false,
            focus,
            _task: None,
        });
        self.reload_branches(cx);
        cx.notify();
    }

    fn close_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_picker = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn reload_branches(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.branch_picker else { return };
        let workdir = picker.workdir.clone();
        let list = cx.background_executor().spawn(async move { grove_git::list_branches(&workdir) });
        picker._task = Some(cx.spawn(async move |this, cx| {
            let result = list.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.branch_picker {
                    match result {
                        Ok(branches) => picker.branches = Some(branches),
                        Err(err) => picker.error = Some(format!("{err:#}")),
                    }
                    cx.notify();
                }
            });
        }));
    }

    /// Avertissements avant de changer de branche dans l'espace direct (specs §9.2).
    fn switch_warnings(&self, key: WorkspaceKey) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(changed) = self.git.get(&key.1).and_then(|g| g.status).map(|s| s.changed).filter(|n| *n > 0) {
            warnings.push(format!(
                "{changed} fichier(s) modifié(s) : ils suivront le changement de branche (git refuse en cas de conflit)."
            ));
        }
        let busy = self.state.workspace(key).into_iter().flat_map(|w| &w.tabs).any(|t| {
            matches!(
                self.claude.sessions.get(&t.id).map(|s| &s.status),
                Some(ClaudeStatus::Working { .. } | ClaudeStatus::NeedsInput { .. })
            )
        });
        if busy {
            warnings.push("Une session Claude est active dans cet espace : elle verra les fichiers changer.".into());
        }
        warnings
    }

    fn activate_item(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.branch_picker else { return };
        match item {
            Item::Branch(branch) if branch.current => self.close_branch_picker(window, cx),
            Item::Branch(branch) => {
                let warnings = self.switch_warnings(picker.workspace);
                if warnings.is_empty() {
                    self.run_op(Op::Switch(branch), window, cx);
                } else if let Some(picker) = &mut self.branch_picker {
                    picker.confirm = Some(Confirm::Switch { branch, warnings });
                    cx.notify();
                }
            }
            Item::Create { name, base } => self.run_op(Op::Create { name, base }, window, cx),
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.branch_picker.as_mut().and_then(|p| p.confirm.take()) else { return };
        match confirm {
            Confirm::Switch { branch, .. } => self.run_op(Op::Switch(branch), window, cx),
            Confirm::Delete { name, force } => self.run_op(Op::Delete { name, force }, window, cx),
        }
    }

    fn run_op(&mut self, op: Op, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.branch_picker else { return };
        let workdir = picker.workdir.clone();
        let workspace = picker.workspace.1;
        picker.busy = true;
        picker.error = None;
        let closes = !matches!(op, Op::Delete { .. });
        if closes {
            self.expected_head_change = Some(workspace);
        }
        let delete_name = match &op {
            Op::Delete { name, force: false } => Some(name.clone()),
            _ => None,
        };
        let work = cx.background_executor().spawn(async move {
            match &op {
                Op::Switch(branch) => grove_git::switch(&workdir, branch),
                Op::Create { name, base } => grove_git::create(&workdir, name, base.as_deref()),
                Op::Delete { name, force } => grove_git::delete(&workdir, name, *force),
            }
        });
        picker._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.refresh_git_status(workspace, cx);
                if this.branch_picker.is_none() {
                    return;
                }
                match result {
                    Ok(()) if closes => return this.close_branch_picker(window, cx),
                    Ok(()) => this.reload_branches(cx),
                    // Branche non fusionnée : on propose de forcer.
                    Err(err) if delete_name.is_some() && err.to_string().contains("not fully merged") => {
                        let name = delete_name.unwrap_or_default();
                        if let Some(picker) = &mut this.branch_picker {
                            picker.confirm = Some(Confirm::Delete { name, force: true });
                        }
                    }
                    Err(err) => {
                        if let Some(picker) = &mut this.branch_picker {
                            picker.error = Some(format!("{err:#}"));
                        }
                    }
                }
                if let Some(picker) = &mut this.branch_picker {
                    picker.busy = false;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn branch_picker_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.branch_picker else { return };
        let keystroke = &event.keystroke;
        let count = picker.items().len();
        match keystroke.key.as_str() {
            "escape" if picker.confirm.is_some() => picker.confirm = None,
            "escape" => return self.close_branch_picker(window, cx),
            "enter" if picker.confirm.is_some() => return self.confirm(window, cx),
            "enter" => {
                if let Some(item) = picker.items().get(picker.selected).cloned() {
                    self.activate_item(item, window, cx);
                }
            }
            "up" => {
                picker.selected = picker.selected.saturating_sub(1);
                picker.scroll.reveal(picker.selected, count);
            }
            "down" => {
                picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
                picker.scroll.reveal(picker.selected, count);
            }
            _ => match picker.query.key_down(keystroke, cx) {
                Edit::Ignored => return,
                Edit::Changed | Edit::Copy { cut: true, .. } => {
                    picker.selected = 0;
                    picker.scroll.reset();
                    picker.confirm = None;
                }
                Edit::Selection | Edit::Copy { .. } => {}
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn is_direct(&self, key: WorkspaceKey) -> bool {
        self.state.workspace(key).is_some_and(|w| w.kind == grove_core::WorkspaceKind::Direct)
    }

    fn branch_row(&self, index: usize, item: &Item, selected: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = list_row(format!("branch-item-{index}"), selected).h(ROW_HEIGHT).on_click(cx.listener({
            let item = item.clone();
            move |this, _, window, cx| this.activate_item(item.clone(), window, cx)
        }));
        match item {
            Item::Branch(branch) => {
                let name = branch.name.clone();
                let deletable = !branch.remote && !branch.current;
                let glyph = match (branch.current, branch.remote) {
                    (true, _) => icon(Icon::Check, theme::accent()),
                    (false, true) => icon(Icon::ArrowDown, theme::text_faint()),
                    (false, false) => icon(Icon::GitBranch, theme::text_faint()),
                };
                // La couleur va sur la ligne : `marquee` peint avec le style de son parent.
                row.text_color(if branch.remote { theme::text_muted() } else { theme::text() })
                    .child(glyph)
                    .child(marquee(SharedString::from(format!("branch-name-{index}")), branch.name.clone()))
                    // Branche suivie : seulement si elle apporte une information (pas `origin/<même nom>`).
                    .children(branch.upstream.clone().filter(|u| *u != format!("origin/{}", branch.name)).map(|u| {
                        div()
                            .flex_none()
                            .max_w(px(150.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .child(u)
                    }))
                    .when(deletable, |d| {
                        d.child(
                            icon_button(format!("delete-branch-{index}"), Icon::Trash)
                                .size(px(20.0))
                                .invisible()
                                .group_hover(SharedString::from(format!("branch-item-{index}")), |s| s.visible())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(picker) = &mut this.branch_picker {
                                        picker.confirm = Some(Confirm::Delete { name: name.clone(), force: false });
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .into_any_element()
            }
            Item::Create { name, base } => row
                .text_color(theme::text())
                .child(icon(Icon::Plus, theme::accent()))
                .child(marquee(
                    SharedString::from(format!("branch-create-{index}")),
                    match base {
                        Some(base) => format!("Créer « {name} » depuis « {base} »"),
                        None => format!("Créer « {name} » depuis la branche courante"),
                    },
                ))
                .into_any_element(),
        }
    }

    /// Pastille de la barre d'onglets : « ⎇ main ↑1 ●3 ».
    pub(crate) fn render_branch_chip(&self, key: WorkspaceKey, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let git = self.git.get(&key.1)?;
        let head = git.head.as_ref()?;
        let status = git.status.as_ref().map(status_text).unwrap_or_default();
        Some(
            div()
                .id("branch-chip")
                .h(px(26.0))
                .px_2()
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .text_sm()
                .text_color(theme::text())
                .hover(|s| s.bg(theme::hover()))
                .cursor_pointer()
                .when(self.branch_picker.is_some(), |d| d.bg(theme::selected()))
                .tooltip(tooltip("Branches · Ctrl+Shift+G"))
                .on_click(cx.listener(|this, _, window, cx| {
                    // Le clic qui vient de fermer le sélecteur (clic extérieur) ne le rouvre pas.
                    if !std::mem::take(&mut this.popover_dismissed) {
                        this.open_branch_picker(window, cx);
                    }
                }))
                .child(icon(Icon::GitBranch, theme::accent()))
                .child(div().flex().max_w(px(200.0)).min_w_0().child(marquee("branch-chip-label", head.label().to_owned())))
                .when(!status.is_empty(), |d| d.child(div().text_xs().text_color(theme::text_faint()).child(status)))
                .into_any_element(),
        )
    }

    pub(crate) fn render_branch_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let picker = self.branch_picker.as_ref()?;
        let items = picker.items();

        let count = items.len();
        let list = div()
            .relative()
            .h(ListScroll::height(count))
            .child(
                uniform_list(
                    "branch-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let Some(picker) = &this.branch_picker else { return Vec::new() };
                        let (items, selected) = (picker.items(), picker.selected);
                        range
                            .filter_map(|index| Some(this.branch_row(index, items.get(index)?, index == selected, cx)))
                            .collect()
                    }),
                )
                .track_scroll(picker.scroll.handle.clone())
                .size_full(),
            )
            .children(picker.scroll.scrollbar(count));

        let confirm = picker.confirm.as_ref().map(|confirm| {
            let (lines, action, style): (Vec<String>, &str, ButtonStyle) = match confirm {
                Confirm::Switch { branch, warnings } => {
                    let mut lines = vec![format!("Passer sur « {} » ?", branch.name)];
                    lines.extend(warnings.iter().cloned());
                    (lines, "Changer quand même", ButtonStyle::Primary)
                }
                Confirm::Delete { name, force: false } => {
                    (vec![format!("Supprimer la branche « {name} » ?")], "Supprimer", ButtonStyle::Danger)
                }
                Confirm::Delete { name, force: true } => (
                    vec![format!("« {name} » n'est pas fusionnée : ses commits seront perdus.")],
                    "Supprimer quand même",
                    ButtonStyle::Danger,
                ),
            };
            // Première ligne : la question ; les suivantes : les avertissements.
            let mut lines = lines.into_iter();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .border_t_1()
                .border_color(theme::border())
                .bg(theme::panel())
                .children(lines.next().map(|l| div().text_color(theme::text()).child(l)))
                .children(lines.map(|l| div().text_xs().text_color(theme::waiting()).child(l)))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .pt_2()
                        .justify_end()
                        .child(button("branch-cancel", "Annuler", ButtonStyle::Secondary).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(picker) = &mut this.branch_picker {
                                    picker.confirm = None;
                                    cx.notify();
                                }
                            },
                        )))
                        .child(
                            button("branch-confirm", action, style)
                                .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                        ),
                )
        });

        let status = match (&picker.branches, picker.busy) {
            (_, true) => Some("git en cours…"),
            (None, _) => Some("Chargement des branches…"),
            (Some(_), _) if items.is_empty() => Some("Aucune branche."),
            _ => None,
        };

        let panel = popover("branch-picker", &picker.focus, px(440.0))
            .on_key_down(cx.listener(Self::branch_picker_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.popover_dismissed = true;
                this.close_branch_picker(window, cx);
            }))
            .child(search_field(&picker.query, "Chercher ou créer une branche…"))
            .child(text_field::input_target(&picker.focus, cx.entity()))
            .child(div().p_1().child(list))
            .children(status.map(|s| div().px_3().py_2().text_color(theme::text_muted()).child(s)))
            .children(picker.error.clone().map(|e| div().px_3().py_2().text_color(theme::danger()).child(e)))
            .children(confirm)
            .when(self.is_direct(picker.workspace), |d| {
                let project = picker.workspace.0;
                d.child(
                    div()
                        .id("promote")
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_color(theme::text_muted())
                        .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_worktree_dialog(project, crate::worktrees::DialogMode::Promote, window, cx)
                        }))
                        .cursor_pointer()
                        .child(icon(Icon::CornerDownRight, theme::text_faint()))
                        .child("Déplacer le travail en cours dans un worktree…"),
                )
            })
            .child(
                popover_footer()
                    .child(hint("↑↓", "naviguer"))
                    .child(hint("Entrée", "valider"))
                    .child(hint("Échap", "fermer")),
            );
        Some(picker.scroll.track_drag(panel, count).into_any_element())
    }

    /// Synchronise l'espace avec son remote (fetch, pull en fast-forward, push),
    /// ou publie la branche si elle n'a pas de branche suivie.
    pub(crate) fn sync_workspace(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        let Some(path) = self.state.projects.iter().flat_map(|p| &p.workspaces).find(|w| w.id == workspace).map(|w| w.path.clone())
        else {
            return;
        };
        let Some(git) = self.git.get_mut(&workspace).filter(|g| !g.syncing) else { return };
        git.syncing = true;
        let work = cx.background_executor().spawn(async move { grove_git::sync(&path) });
        git.sync_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(git) = this.git.get_mut(&workspace) {
                    git.syncing = false;
                }
                match result {
                    Ok(()) => {
                        if this.notice.as_ref().is_some_and(|n| n.workspace == workspace && !n.suggest_worktree) {
                            this.notice = None;
                        }
                    }
                    Err(err) => {
                        this.notice = Some(Notice {
                            workspace,
                            text: format!("Synchronisation impossible : {err:#}"),
                            suggest_worktree: false,
                        })
                    }
                }
                this.refresh_git_status(workspace, cx);
                if this.branch_picker.as_ref().is_some_and(|p| p.workspace.1 == workspace) {
                    this.reload_branches(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Bouton de synchronisation de la barre d'onglets, comme celui de VS Code :
    /// « ↻ 2↓ 1↑ », ou « publier » pour une branche sans branche suivie.
    pub(crate) fn render_sync_button(&self, key: WorkspaceKey, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let git = self.git.get(&key.1)?;
        let status = git.status?;
        if !status.has_remote || git.head.as_ref().is_some_and(|h| !matches!(h, grove_git::Head::Branch(_))) {
            return None;
        }
        let workspace = key.1;
        let (glyph, hint) = match (git.syncing, status.has_upstream) {
            (true, _) => (Icon::Refresh, "Synchronisation…".to_owned()),
            (false, false) => (Icon::CloudUpload, "Publier la branche".to_owned()),
            (false, true) => (
                Icon::Refresh,
                format!(
                    "Synchroniser : récupérer, tirer {} et pousser {} commit(s)",
                    status.behind, status.ahead
                ),
            ),
        };
        let color = if status.ahead > 0 || status.behind > 0 { theme::accent() } else { theme::text_muted() };
        let glyph = sized_icon(glyph, px(14.0), color);
        let glyph = if git.syncing {
            glyph
                .with_animation(
                    "sync-spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };
        let count = |arrow: &str, n: u32| (n > 0).then(|| div().child(format!("{n}{arrow}")));
        Some(
            div()
                .id("sync-button")
                .h(px(26.0))
                .px_1p5()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .rounded_md()
                .text_xs()
                .text_color(theme::text_muted())
                .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
                .cursor_pointer()
                .tooltip(tooltip(hint))
                .on_click(cx.listener(move |this, _, _, cx| this.sync_workspace(workspace, cx)))
                .child(glyph)
                .children(count("↓", status.behind))
                .children(count("↑", status.ahead))
                .into_any_element(),
        )
    }
}
