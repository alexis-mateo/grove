//! Sidebar : projets → espaces de travail. Redimensionnable par son bord droit.

use gpui::{
    ClickEvent, Context, CursorStyle, DragMoveEvent, FontWeight, MouseButton, Pixels, Render, SharedString,
    Window, div, prelude::*, px,
};
use grove_claude::ClaudeStatus;
use grove_core::{Project, ProjectId, Workspace, WorkspaceKind};

use crate::grove_view::{AddProject, GroveView, ToggleSidebar};
use crate::marquee::marquee;
use crate::theme;
use crate::ui::{Icon, dot, icon, icon_button, sized_icon, tooltip};
use crate::window_frame::{TOP_BAR_HEIGHT, drag_region};

pub const DEFAULT_SIDEBAR_WIDTH: Pixels = px(248.0);
const MIN_SIDEBAR_WIDTH: Pixels = px(180.0);
const MAX_SIDEBAR_WIDTH: Pixels = px(520.0);
/// Largeur de la poignée de redimensionnement, à cheval sur le bord.
const HANDLE_WIDTH: Pixels = px(6.0);
/// Délai entre le dernier mouvement de la poignée et l'écriture de `state.json`.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// Bord de la sidebar en cours de glisser.
#[derive(Clone)]
pub struct DraggedSidebar;

impl Render for DraggedSidebar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Projet en cours de glisser-déposer, pour réordonner la sidebar.
#[derive(Clone)]
pub struct DraggedProject {
    project: ProjectId,
    name: SharedString,
}

impl Render for DraggedProject {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(28.0))
            .px_3()
            .flex()
            .items_center()
            .rounded_md()
            .bg(theme::selected())
            .border_1()
            .border_color(theme::accent().opacity(0.6))
            .shadow_lg()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::text())
            .child(self.name.clone())
    }
}

impl GroveView {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .h(TOP_BAR_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .gap_0p5()
            .pl_2()
            .pr_1p5()
            .child(drag_region(
                div()
                    .id("sidebar-drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .pl_1p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_faint())
                    .child("Projets"),
            ))
            .child(
                icon_button("add-project", Icon::FolderPlus)
                    .tooltip(tooltip("Ajouter un projet · Ctrl+Shift+O"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_project(&AddProject, window, cx))),
            )
            .child(
                icon_button("hide-sidebar", Icon::PanelLeft)
                    .tooltip(tooltip("Masquer la sidebar · Ctrl+Shift+B"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx))),
            );

        let projects: Vec<_> = self
            .state
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| self.render_project(index, project, cx))
            .collect();

        div()
            .w(self.sidebar_width)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::border())
            .child(header)
            .child(
                div()
                    .id("projects")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_2()
                    .pt_1()
                    .pb_3()
                    .text_sm()
                    .children(projects),
            )
            .children(self.render_usage(cx))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        icon_button("open-settings", Icon::Settings)
                            .when(self.settings.is_some(), |b| b.bg(theme::selected()))
                            .tooltip(tooltip("Paramètres · Ctrl+,"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.settings.is_some() {
                                    this.close_settings(window, cx);
                                } else {
                                    this.open_settings(None, None, window, cx);
                                }
                            })),
                    )
                    .child({
                        let requested = self.github.inbox.as_ref().map_or(0, |inbox| {
                            inbox.to_review.iter().filter(|p| p.review_state() == grove_github::ReviewState::Requested).count()
                        });
                        icon_button("open-pull-requests", Icon::GitPullRequest)
                            .when(self.github.panel.is_some(), |b| b.bg(theme::selected()))
                            .when(requested > 0, |b| b.w_auto().px_1p5().gap_1().child(
                                div().text_xs().text_color(theme::waiting()).child(requested.to_string()),
                            ))
                            .tooltip(tooltip(match requested {
                                0 => "Pull requests · Ctrl+Shift+P".to_owned(),
                                n => format!("Pull requests · {n} review(s) demandée(s) · Ctrl+Shift+P"),
                            }))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_pull_requests(window, cx)))
                    })
                    .child(
                        icon_button("open-shortcuts", Icon::Keyboard)
                            .tooltip(tooltip("Raccourcis clavier"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(Some(crate::settings::Section::Shortcuts), None, window, cx)
                            })),
                    ),
            )
    }

    /// Poignée sur le bord droit de la sidebar. Posée par-dessus tout le reste
    /// (dans un parent `relative`) pour rester saisissable des deux côtés du bord.
    pub(crate) fn render_sidebar_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize")
            .group("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .left(self.sidebar_width - HANDLE_WIDTH / 2.0)
            .w(HANDLE_WIDTH)
            .flex()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_drag(DraggedSidebar, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dragged.clone())
            })
            // Double-clic : largeur par défaut.
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.sidebar_width = DEFAULT_SIDEBAR_WIDTH;
                    this.state.sidebar_width = None;
                    this.save();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .w(px(2.0))
                    .h_full()
                    .group_hover("sidebar-resize", |s| s.bg(theme::accent().opacity(0.6))),
            )
    }

    /// Suit le glisser de la poignée. `event.bounds` : la vue racine, dont le bord
    /// gauche est celui de la sidebar.
    pub(crate) fn resize_sidebar(&mut self, event: &DragMoveEvent<DraggedSidebar>, cx: &mut Context<Self>) {
        let max = MAX_SIDEBAR_WIDTH.min(event.bounds.size.width - px(320.0)).max(MIN_SIDEBAR_WIDTH);
        let width = (event.event.position.x - event.bounds.left()).clamp(MIN_SIDEBAR_WIDTH, max).round();
        if width != self.sidebar_width {
            self.sidebar_width = width;
            self.state.sidebar_width = Some(f32::from(width));
            self.save_sidebar_width_soon(cx);
            cx.notify();
        }
    }

    /// Enregistre la largeur peu après le dernier mouvement : sans attendre le
    /// relâchement, qui n'arrive pas si le pointeur sort de la fenêtre.
    fn save_sidebar_width_soon(&mut self, cx: &mut Context<Self>) {
        self.sidebar_save = Some(cx.spawn(async move |this, cx| {
            gpui::Timer::after(SAVE_DELAY).await;
            let _ = this.update(cx, |this, _| this.save());
        }));
    }

    fn render_project(&self, index: usize, project: &Project, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = project.id;
        let is_git = project.workspaces.iter().any(|w| self.git.contains_key(&w.id));
        let group: SharedString = format!("project-{id}").into();

        let row = div()
            .id(SharedString::from(format!("project-row-{id}")))
            .group(group.clone())
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_1p5()
            .px_1p5()
            .rounded_md()
            .hover(|s| s.bg(theme::hover()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(id, cx)))
            .on_drag(
                DraggedProject { project: id, name: project.name.clone().into() },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            )
            .child(sized_icon(
                if project.collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                px(12.0),
                theme::text_faint(),
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(marquee(SharedString::from(format!("project-name-{id}")), project.name.clone())),
            )
            .when(!is_git, |d| d.child(div().flex_none().text_xs().text_color(theme::text_faint()).child("pas git")))
            .when(is_git, |d| {
                d.child(
                    icon_button(SharedString::from(format!("new-workspace-{id}")), Icon::Plus)
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .tooltip(tooltip("Nouvel espace de travail · Ctrl+Shift+N"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_worktree_dialog(id, crate::worktrees::DialogMode::Create, window, cx);
                        })),
                )
            })
            .child(
                icon_button(SharedString::from(format!("remove-{id}")), Icon::X)
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .tooltip(tooltip("Retirer le projet de Grove"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.remove_project(id, window, cx);
                    })),
            );

        // Tout le bloc (projet et ses espaces) reçoit le dépôt : le projet glissé
        // prend sa place.
        div()
            .id(SharedString::from(format!("project-{id}")))
            .flex()
            .flex_col()
            .gap_px()
            .rounded_md()
            .border_1()
            .border_color(gpui::transparent_black())
            .drag_over::<DraggedProject>(|s, _, _, _| s.bg(theme::hover()).border_color(theme::accent().opacity(0.5)))
            .on_drop(cx.listener(move |this, dragged: &DraggedProject, _, cx| {
                this.move_project_to(dragged.project, index, cx);
            }))
            .child(row)
            .when(!project.collapsed, |d| {
                let rows: Vec<_> =
                    project.workspaces.iter().map(|w| self.render_workspace(project, w, cx)).collect();
                d.children(rows)
            })
            .into_any_element()
    }

    fn render_workspace(
        &self,
        project: &Project,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = (project.id, workspace.id);
        let active = self.state.active_workspace == Some(key);
        let head = self.git.get(&workspace.id).and_then(|g| g.head.as_ref());
        let label: SharedString = match (&workspace.kind, head) {
            (WorkspaceKind::Direct, Some(head)) => head.label().to_owned().into(),
            (WorkspaceKind::Direct, None) => "dossier".into(),
            (WorkspaceKind::Worktree { .. }, Some(head)) => head.label().to_owned().into(),
            (WorkspaceKind::Worktree { branch, .. }, None) => branch.clone().into(),
        };

        // État agrégé des sessions Claude de l'espace : l'attente prime sur le travail.
        let statuses: Vec<_> = workspace
            .tabs
            .iter()
            .filter_map(|t| self.claude.sessions.get(&t.id).map(|s| &s.status))
            .collect();
        let waiting = statuses.iter().filter(|s| matches!(s, ClaudeStatus::NeedsInput { .. })).count();
        let working = statuses.iter().any(|s| matches!(s, ClaudeStatus::Working { .. }));
        let meta = if waiting > 0 {
            let text = if waiting == 1 { "attend".to_owned() } else { format!("{waiting} attendent") };
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_color(theme::waiting())
                .child(dot(theme::waiting()))
                .child(text)
        } else if working {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_color(theme::text_muted())
                .child(dot(theme::accent()))
                .child("en cours")
        } else if workspace.tabs.len() > 1 {
            div().text_color(theme::text_faint()).child(workspace.tabs.len().to_string())
        } else {
            div()
        };

        // Icône de l'espace : maison pour le checkout principal, branche pour un
        // worktree (✳ pour ceux créés par Claude Code), dossier hors git.
        let is_worktree = matches!(workspace.kind, WorkspaceKind::Worktree { .. });
        let (glyph, hint) = match (&workspace.kind, head) {
            (WorkspaceKind::Direct, Some(_)) => (Icon::House, "Checkout principal"),
            (WorkspaceKind::Direct, None) => (Icon::Folder, "Dossier hors git"),
            (WorkspaceKind::Worktree { .. }, _) if workspace.path.components().any(|c| c.as_os_str() == ".claude") => {
                (Icon::Claude, "Worktree créé par Claude Code")
            }
            (WorkspaceKind::Worktree { .. }, _) => (Icon::GitBranch, "Worktree"),
        };
        let leading = div()
            .id(SharedString::from(format!("workspace-icon-{}", workspace.id)))
            .flex_none()
            .tooltip(tooltip(format!("{hint} · {}", workspace.path.display())))
            .child(icon(glyph, if active { theme::text() } else { theme::text_faint() }));

        // PR ouverte de la branche, avec l'état de sa CI.
        let pull = match head {
            Some(grove_git::Head::Branch(branch)) => self.workspace_pull(project.id, branch),
            _ => None,
        };
        let pull = pull.map(|pr| {
            let (color, ci) = pr.checks.map_or((theme::text_faint(), "pas de CI"), crate::github::checks_look);
            let ci = if pr.checks.is_some() { format!("CI {ci}") } else { ci.to_owned() };
            div()
                .id(SharedString::from(format!("workspace-pull-{}", workspace.id)))
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(theme::text_faint())
                .tooltip(tooltip(format!("PR #{} · {} · {ci}", pr.number, pr.title)))
                .child(dot(color))
                .child(format!("#{}", pr.number))
        });

        let group: SharedString = format!("workspace-row-{}", workspace.id).into();
        div()
            .id(SharedString::from(format!("workspace-{}", workspace.id)))
            .group(group.clone())
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_2()
            .pl(px(24.0))
            .pr_1p5()
            .rounded_md()
            .cursor_pointer()
            .when(active, |d| d.bg(theme::selected()).text_color(theme::text()))
            .when(!active, |d| d.text_color(theme::text_muted()).hover(|s| s.bg(theme::hover()).text_color(theme::text())))
            .on_click(cx.listener(move |this, _, window, cx| this.activate_workspace(key, window, cx)))
            .child(leading)
            .child(marquee(SharedString::from(format!("workspace-label-{}", workspace.id)), label))
            .children(pull)
            .child(meta.flex_none().text_xs())
            .when(is_worktree, |d| {
                d.child(
                    icon_button(SharedString::from(format!("remove-workspace-{}", workspace.id)), Icon::X)
                        .size(px(20.0))
                        .invisible()
                        .group_hover(group, |s| s.visible())
                        .tooltip(tooltip("Retirer cet espace"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_remove_dialog(key, window, cx);
                        })),
                )
            })
            .into_any_element()
    }
}
