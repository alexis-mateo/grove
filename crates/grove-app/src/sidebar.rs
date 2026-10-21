//! Sidebar : projets → espaces de travail.

use gpui::{Context, MouseButton, SharedString, div, prelude::*, px};
use grove_claude::ClaudeStatus;
use grove_core::{Project, Workspace, WorkspaceKind};

use crate::grove_view::{AddProject, GroveView};
use crate::marquee::marquee;
use crate::theme;

pub const SIDEBAR_WIDTH: gpui::Pixels = px(240.0);

impl GroveView {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .h(px(32.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .text_xs()
            .text_color(theme::text_muted())
            .child("PROJETS")
            .child(
                icon_button("add-project", "+")
                    .on_click(cx.listener(|this, _, window, cx| this.add_project(&AddProject, window, cx))),
            );

        let projects: Vec<_> = self
            .state
            .projects
            .iter()
            .map(|project| self.render_project(project, cx))
            .collect();

        div()
            .w(SIDEBAR_WIDTH)
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
                    .text_sm()
                    .children(projects),
            )
            .children(self.render_usage(cx))
    }

    fn render_project(&self, project: &Project, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = project.id;
        let is_git = project.workspaces.iter().any(|w| self.git.contains_key(&w.id));
        let group: SharedString = format!("project-{id}").into();

        let row = div()
            .id(SharedString::from(format!("project-row-{id}")))
            .group(group.clone())
            .h(px(28.0))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .hover(|s| s.bg(theme::hover()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(id, cx)))
            .child(
                div()
                    .w(px(14.0))
                    .text_color(theme::text_muted())
                    .child(if project.collapsed { "▸" } else { "▾" }),
            )
            .child(marquee(SharedString::from(format!("project-name-{id}")), project.name.clone()))
            .when(!is_git, |d| d.child(div().flex_none().text_xs().text_color(theme::text_muted()).child("pas git")))
            .when(is_git, |d| {
                d.child(
                    icon_button(SharedString::from(format!("new-workspace-{id}")), "+")
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_worktree_dialog(id, crate::worktrees::DialogMode::Create, window, cx);
                        })),
                )
            })
            .child(
                icon_button(SharedString::from(format!("remove-{id}")), "×")
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.remove_project(id, window, cx);
                    })),
            );

        div()
            .flex()
            .flex_col()
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
        let tabs = match workspace.tabs.len() {
            0 => String::new(),
            1 => "1 onglet".to_owned(),
            n => format!("{n} onglets"),
        };
        // État agrégé des sessions Claude de l'espace : l'attente prime sur le travail.
        let statuses: Vec<_> = workspace
            .tabs
            .iter()
            .filter_map(|t| self.claude.sessions.get(&t.id).map(|s| &s.status))
            .collect();
        let waiting = statuses.iter().filter(|s| matches!(s, ClaudeStatus::NeedsInput { .. })).count();
        let working = statuses.iter().any(|s| matches!(s, ClaudeStatus::Working { .. }));
        let (dot, dot_color, detail, detail_color) = if waiting > 0 {
            let detail = if waiting == 1 { "claude attend ta réponse".to_owned() } else { format!("{waiting} sessions attendent") };
            ("◐", theme::waiting(), detail, theme::waiting())
        } else if working {
            ("●", theme::accent(), "claude travaille".to_owned(), theme::text_muted())
        } else if workspace.tabs.is_empty() {
            ("○", theme::text_muted(), tabs, theme::text_muted())
        } else {
            ("●", theme::text_muted(), tabs, theme::text_muted())
        };

        let group: SharedString = format!("workspace-row-{}", workspace.id).into();
        let is_worktree = matches!(workspace.kind, WorkspaceKind::Worktree { .. });
        div()
            .id(SharedString::from(format!("workspace-{}", workspace.id)))
            .group(group.clone())
            .h(px(26.0))
            .flex()
            .items_center()
            .gap_2()
            .pl(px(26.0))
            .pr_3()
            .cursor_pointer()
            .when(active, |d| d.bg(theme::selected()))
            .hover(|s| s.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.activate_workspace(key, window, cx)))
            .child(div().text_xs().text_color(dot_color).child(dot))
            // Worktree : icône de branche (✳ pour ceux créés par Claude Code).
            .when(is_worktree, |d| {
                let claude = workspace.path.components().any(|c| c.as_os_str() == ".claude");
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(if claude { "✳" } else { "⎇" }),
                )
            })
            .child(marquee(SharedString::from(format!("workspace-label-{}", workspace.id)), label))
            // Checkout principal : pastille qui le distingue des worktrees.
            .when(!is_worktree && head.is_some(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme::accent().opacity(0.5))
                        .text_size(px(10.0))
                        .text_color(theme::accent())
                        .child("principal"),
                )
            })
            .child(
                div()
                    .flex_none()
                    .max_w(px(130.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(detail_color)
                    .child(detail),
            )
            .when(is_worktree, |d| {
                d.child(
                    icon_button(SharedString::from(format!("remove-workspace-{}", workspace.id)), "×")
                        .invisible()
                        .group_hover(group, |s| s.visible())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_remove_dialog(key, window, cx);
                        })),
                )
            })
            .into_any_element()
    }
}

pub(crate) fn icon_button(id: impl Into<gpui::ElementId>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_color(theme::text_muted())
        .hover(|s| s.bg(theme::selected()).text_color(theme::text()))
        .cursor_pointer()
        .child(label)
}
