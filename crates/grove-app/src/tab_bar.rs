//! Barre d'onglets de l'espace de travail actif. Elle sert aussi de barre de
//! titre : son espace libre déplace la fenêtre, les boutons de la fenêtre sont
//! à droite.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, KeyDownEvent, MouseButton, Render, SharedString, Transformation, Window, div,
    ease_in_out, percentage, prelude::*, px, relative,
};
use grove_claude::ClaudeStatus;
use grove_core::{ClaudeView, TabId, TabKind};
use grove_term::Progress;

use crate::claude::status_badge;
use crate::grove_view::{GroveView, NewTab, ToggleSidebar};
use crate::marquee::marquee;
use crate::text_field::{self, Edit};
use crate::theme;
use crate::ui::{Icon, icon, icon_button, sized_icon, tooltip, vertical_divider};
use crate::window_frame::{TOP_BAR_HEIGHT, drag_region, window_controls};

const TAB_HEIGHT: gpui::Pixels = px(28.0);
/// Part de la barre occupée par le segment qui va et vient quand l'avancement est inconnu.
const INDETERMINATE_WIDTH: f32 = 0.35;

/// Onglet en cours de glisser-déposer.
#[derive(Clone)]
pub struct DraggedTab {
    tab: TabId,
    label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(TAB_HEIGHT)
            .px_3()
            .flex()
            .items_center()
            .rounded_md()
            .bg(theme::selected())
            .border_1()
            .border_color(theme::accent().opacity(0.6))
            .shadow_lg()
            .text_sm()
            .text_color(theme::text())
            .child(self.label.clone())
    }
}

impl GroveView {
    pub(crate) fn render_tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = div()
            .h(TOP_BAR_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl_2()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::border())
            .when(!self.sidebar_visible, |d| {
                d.child(
                    icon_button("show-sidebar", Icon::PanelLeft)
                        .tooltip(tooltip("Afficher la sidebar · Ctrl+Shift+B"))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx))),
                )
            });

        let workspace = self.state.active_workspace.and_then(|key| Some((key, self.state.workspace(key)?)));
        let bar = match workspace {
            Some((key, workspace)) => {
                let active = workspace.active_tab;
                let tabs: Vec<_> = workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| self.render_tab(key, index, tab, active == Some(tab.id), cx))
                    .collect();
                let claude_tab = active.is_some_and(|tab| self.claude.sessions.contains_key(&tab));
                let native = active.and_then(|tab| self.claude_view(tab)) == Some(ClaudeView::Native);

                let sync = self.render_sync_button(key, cx);
                bar.children(self.render_branch_chip(key, cx).map(|chip| {
                    div().flex().flex_none().items_center().child(chip).children(sync).child(vertical_divider())
                }))
                .child(div().flex().items_center().gap_1().min_w_0().flex_shrink().children(tabs))
                .child(
                    icon_button("new-tab", Icon::Plus)
                        .tooltip(tooltip("Nouveau terminal · Ctrl+Shift+T · clic droit : profils"))
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, window, cx| this.open_profile_picker(window, cx)),
                        ),
                )
                .child(
                    icon_button("new-tab-profile", Icon::ChevronDown)
                        .w(px(16.0))
                        .when(self.profile_picker.is_some(), |b| b.bg(theme::selected()))
                        .tooltip(tooltip("Ouvrir un terminal avec un profil (SSH…)"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            // Le clic qui vient de fermer le sélecteur (clic extérieur) ne le rouvre pas.
                            if !std::mem::take(&mut this.popover_dismissed) {
                                this.open_profile_picker(window, cx);
                            }
                        })),
                )
                .child(
                    div()
                        .id("new-claude")
                        .group("new-claude")
                        .h(px(26.0))
                        .px_2()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1p5()
                        .rounded_md()
                        .text_sm()
                        .text_color(theme::text_muted())
                        .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
                        .cursor_pointer()
                        .tooltip(tooltip("Nouvelle session Claude · Ctrl+Shift+A"))
                        .on_click(cx.listener(|this, _, window, cx| this.new_claude_tab(None, window, cx)))
                        .child(icon(Icon::Claude, theme::accent()))
                        .child("Claude"),
                )
                .child(drag_region(div().id("tab-bar-drag").flex_1().min_w(px(24.0)).h_full()))
                .child(
                    icon_button("history-button", Icon::History)
                        .when(self.claude.history.is_some(), |b| b.bg(theme::selected()))
                        .tooltip(tooltip("Sessions Claude de cet espace · Ctrl+Shift+H"))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_history(window, cx))),
                )
                .when_some(active.and_then(|tab| self.claude_view(tab)), |d, view| {
                    let (glyph, label) = match view {
                        ClaudeView::Terminal => (Icon::Claude, "Passer à la vue native · Ctrl+Shift+M"),
                        ClaudeView::Native => (Icon::Terminal, "Passer au terminal (TUI officiel) · Ctrl+Shift+M"),
                    };
                    d.child(
                        icon_button("claude-view-button", glyph)
                            .tooltip(tooltip(label))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_claude_view(window, cx))),
                    )
                })
                .when(claude_tab && !native, |d| {
                    d.child(
                        icon_button("session-panel-button", Icon::PanelRight)
                            .when(self.claude.panel_visible, |b| b.bg(theme::selected()))
                            .tooltip(tooltip("Détails de la session · Ctrl+Shift+I"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.claude.panel_visible = !this.claude.panel_visible;
                                cx.notify();
                            })),
                    )
                })
            }
            None => bar.child(drag_region(div().id("tab-bar-drag").flex_1().h_full())),
        };

        bar.when_some(window_controls(window), |d, controls| d.child(vertical_divider()).child(controls))
            .when(window_controls(window).is_none(), |d| d.pr_2())
    }

    fn render_tab(
        &self,
        key: crate::grove_view::WorkspaceKey,
        index: usize,
        tab: &grove_core::Tab,
        is_active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = tab.id;
        let label = self.tab_label(id, cx);
        let renaming = self.rename.as_ref().filter(|r| r.tab == id);
        let group: SharedString = format!("tab-{id}").into();

        let leading = match (&tab.kind, self.claude.sessions.get(&id)) {
            // Tourne lentement tant que Claude travaille.
            (TabKind::Claude { .. }, Some(session)) if matches!(session.status, ClaudeStatus::Working { .. }) => {
                icon(Icon::Claude, status_badge(&session.status).1)
                    .with_animation(
                        SharedString::from(format!("tab-working-{id}")),
                        Animation::new(Duration::from_millis(2400)).repeat(),
                        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                    )
                    .into_any_element()
            }
            (TabKind::Claude { .. }, Some(session)) => {
                icon(Icon::Claude, status_badge(&session.status).1).into_any_element()
            }
            (TabKind::Claude { .. }, None) => icon(Icon::Claude, theme::text_faint()).into_any_element(),
            (TabKind::Terminal { .. }, _) => {
                let glyph = self.tab_profile(id).map_or(Icon::Terminal, |p| crate::profiles::profile_icon(&p.profile));
                icon(glyph, if is_active { theme::text_muted() } else { theme::text_faint() }).into_any_element()
            }
        };

        let title = match renaming {
            Some(rename) => div()
                .track_focus(&rename.focus)
                .on_key_down(cx.listener(Self::rename_key_down))
                .relative()
                .flex()
                .px_1()
                .rounded_sm()
                .bg(theme::background())
                .border_1()
                .border_color(theme::accent())
                .text_color(theme::text())
                .child(rename.field.render(""))
                .child(text_field::input_target(&rename.focus, cx.entity()))
                .into_any_element(),
            None => div()
                .flex()
                .min_w_0()
                .child(marquee(SharedString::from(format!("tab-label-{id}")), label.clone()).flex_grow())
                .into_any_element(),
        };

        // Celle du premier volet qui en affiche une.
        let progress = tab.pane_ids().iter().find_map(|pane| self.terminals.get(pane)?.read(cx).progress());

        let close_group = SharedString::from(format!("close-{id}"));
        let close = div()
            .id(close_group.clone())
            .group(close_group.clone())
            .size(px(18.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .hover(|s| s.bg(theme::border_strong()))
            .when(!is_active, |b| b.invisible().group_hover(group.clone(), |s| s.visible()))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.close_tab(key, id, window, cx);
            }))
            .child(
                sized_icon(Icon::X, px(12.0), theme::text_muted())
                    .group_hover(close_group, |s| s.text_color(theme::text())),
            );

        div()
            .id(SharedString::from(format!("tab-{id}")))
            .group(group)
            .relative()
            .h(TAB_HEIGHT)
            .max_w(px(220.0))
            .min_w(px(64.0))
            .flex()
            .flex_shrink()
            .items_center()
            .gap_1p5()
            .pl_2p5()
            .pr_1()
            .rounded_md()
            .border_1()
            .text_sm()
            .cursor_pointer()
            .when(is_active, |d| d.bg(theme::selected()).border_color(theme::border_strong()).text_color(theme::text()))
            .when(!is_active, |d| {
                d.border_color(gpui::transparent_black())
                    .text_color(theme::text_muted())
                    .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
            })
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    this.start_rename(id, window, cx);
                } else {
                    this.activate_tab(key, id, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close_tab(key, id, window, cx)),
            )
            .on_drag(DraggedTab { tab: id, label }, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTab>(|s, _, _, _| s.bg(theme::hover()).border_color(theme::accent().opacity(0.5)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab_to(dragged.tab, index, cx);
            }))
            .child(leading)
            .child(title)
            .child(close)
            .children(progress.map(|progress| progress_bar(id, progress)))
            .into_any_element()
    }

    /// Vue d'un onglet Claude ; `None` pour un terminal.
    fn claude_view(&self, tab: TabId) -> Option<ClaudeView> {
        let key = self.locate_tab(tab)?;
        match self.state.workspace(key)?.tabs.iter().find(|t| t.id == tab)?.kind {
            TabKind::Claude { view, .. } => Some(view),
            TabKind::Terminal { .. } => None,
        }
    }

    pub(crate) fn rename_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "enter" => self.finish_rename(true, window, cx),
            "escape" => self.finish_rename(false, window, cx),
            _ => {
                let Some(rename) = &mut self.rename else { return };
                if rename.field.key_down(keystroke, cx) == Edit::Ignored {
                    return;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Barre de progression du programme (OSC 9;4, celle de Claude Code), au pied de l'onglet.
fn progress_bar(id: TabId, progress: Progress) -> impl IntoElement {
    let color = match progress {
        Progress::Error(_) => theme::danger(),
        Progress::Paused(_) => theme::waiting(),
        Progress::Normal(_) | Progress::Indeterminate => theme::accent(),
    };
    // Une erreur ou une pause sans avancement se voit quand même : barre pleine.
    let value = match progress {
        Progress::Error(0) | Progress::Paused(0) => Some(100),
        progress => progress.value(),
    };
    let track = div()
        .absolute()
        .left_2()
        .right_2()
        .bottom(px(2.0))
        .h(px(2.0))
        .rounded_full()
        .overflow_hidden()
        .bg(theme::border());
    let fill = div().absolute().top_0().h_full().rounded_full().bg(color);
    match value {
        Some(value) => track.child(fill.left_0().w(relative(value as f32 / 100.0))),
        None => track.child(fill.w(relative(INDETERMINATE_WIDTH)).with_animation(
            SharedString::from(format!("tab-progress-{id}")),
            Animation::new(Duration::from_millis(1400)).repeat().with_easing(ease_in_out),
            |fill, delta| fill.left(relative(delta * (1.0 + INDETERMINATE_WIDTH) - INDETERMINATE_WIDTH)),
        )),
    }
}
