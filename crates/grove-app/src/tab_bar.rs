//! Barre d'onglets de l'espace de travail actif. Elle sert aussi de barre de
//! titre : son espace libre déplace la fenêtre, les boutons de la fenêtre sont
//! à droite.

use gpui::{
    Context, KeyDownEvent, MouseButton, Render, SharedString, Window, div, prelude::*, px,
};
use grove_core::{TabId, TabKind};

use crate::claude::status_badge;
use crate::grove_view::{GroveView, NewTab, ToggleSidebar};
use crate::marquee::marquee;
use crate::theme;
use crate::ui::{Icon, icon, icon_button, sized_icon, tooltip, vertical_divider};
use crate::window_frame::{TOP_BAR_HEIGHT, drag_region, window_controls};

const TAB_HEIGHT: gpui::Pixels = px(28.0);

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

                bar.children(self.render_branch_chip(key, cx).map(|chip| {
                    div().flex().flex_none().items_center().child(chip).child(vertical_divider())
                }))
                .child(div().flex().items_center().gap_1().min_w_0().flex_shrink().children(tabs))
                .child(
                    icon_button("new-tab", Icon::Plus)
                        .tooltip(tooltip("Nouveau terminal · Ctrl+Shift+T"))
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx))),
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
                .when(claude_tab, |d| {
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
            (TabKind::Claude { .. }, Some(session)) => icon(Icon::Claude, status_badge(&session.status).1),
            (TabKind::Claude { .. }, None) => icon(Icon::Claude, theme::text_faint()),
            (TabKind::Terminal, _) => icon(Icon::Terminal, if is_active { theme::text_muted() } else { theme::text_faint() }),
        };

        let title = match renaming {
            Some(rename) => div()
                .track_focus(&rename.focus)
                .on_key_down(cx.listener(Self::rename_key_down))
                .flex()
                .px_1()
                .rounded_sm()
                .bg(theme::background())
                .border_1()
                .border_color(theme::accent())
                .text_color(theme::text())
                .child(rename.text.clone())
                .child(div().text_color(theme::accent()).child("▏"))
                .into_any_element(),
            None => div()
                .flex()
                .min_w_0()
                .child(marquee(SharedString::from(format!("tab-label-{id}")), label.clone()).flex_grow())
                .into_any_element(),
        };

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
            .into_any_element()
    }

    fn rename_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "enter" => self.finish_rename(true, window, cx),
            "escape" => self.finish_rename(false, window, cx),
            "backspace" => {
                if let Some(rename) = &mut self.rename {
                    rename.text.pop();
                }
            }
            _ if keystroke.modifiers.control || keystroke.modifiers.platform => return,
            _ => {
                let (Some(rename), Some(text)) = (&mut self.rename, &keystroke.key_char) else {
                    return;
                };
                rename.text.push_str(text);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}
