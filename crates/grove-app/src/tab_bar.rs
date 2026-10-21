//! Barre d'onglets de l'espace de travail actif.

use gpui::{
    Context, KeyDownEvent, MouseButton, Render, SharedString, Window, div, prelude::*, px,
};
use grove_core::TabId;

use crate::grove_view::{GroveView, NewTab};
use crate::claude::status_badge;
use crate::sidebar::icon_button;
use crate::marquee::marquee;
use crate::theme;

pub const TAB_BAR_HEIGHT: gpui::Pixels = px(32.0);

/// Onglet en cours de glisser-déposer.
#[derive(Clone)]
pub struct DraggedTab {
    tab: TabId,
    label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_sm()
            .bg(theme::selected())
            .border_1()
            .border_color(theme::accent())
            .text_sm()
            .text_color(theme::text())
            .child(self.label.clone())
    }
}

impl GroveView {
    pub(crate) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(key) = self.state.active_workspace else {
            return div();
        };
        let Some(workspace) = self.state.workspace(key) else {
            return div();
        };
        let active = workspace.active_tab;

        let branch_chip = self.render_branch_chip(key, cx);
        let tabs = workspace.tabs.iter().enumerate().map(|(index, tab)| {
            let id = tab.id;
            let is_active = active == Some(id);
            let label = self.tab_label(id, cx);
            let renaming = self.rename.as_ref().filter(|r| r.tab == id);
            let group: SharedString = format!("tab-{id}").into();

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
                    .child(rename.text.clone())
                    .child(div().text_color(theme::accent()).child("▏"))
                    .into_any_element(),
                None => div()
                    .flex()
                    .max_w(px(220.0))
                    .min_w_0()
                    .child(marquee(SharedString::from(format!("tab-label-{id}")), label.clone()).flex_grow())
                    .into_any_element(),
            };

            div()
                .id(SharedString::from(format!("tab-{id}")))
                .group(group.clone())
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .pl_3()
                .pr_1()
                .border_r_1()
                .border_color(theme::border())
                .text_sm()
                .cursor_pointer()
                .when(is_active, |d| d.bg(theme::background()).text_color(theme::text()))
                .when(!is_active, |d| d.text_color(theme::text_muted()).hover(|s| s.bg(theme::hover())))
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
                .drag_over::<DraggedTab>(|s, _, _, _| s.bg(theme::selected()))
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.move_tab_to(dragged.tab, index, cx);
                }))
                .children(self.claude.sessions.get(&id).map(|session| {
                    let (glyph, color, _) = status_badge(&session.status);
                    div().text_xs().text_color(color).child(glyph)
                }))
                .child(title)
                .child(
                    icon_button(SharedString::from(format!("close-{id}")), "×")
                        .when(!is_active, |b| b.invisible().group_hover(group, |s| s.visible()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(key, id, window, cx);
                        })),
                )
        });

        div()
            .h(TAB_BAR_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::border())
            .children(branch_chip)
            .children(tabs)
            .child(
                div()
                    .px_1()
                    .flex()
                    .gap_1()
                    .child(
                        icon_button("new-tab", "+")
                            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx))),
                    )
                    .child(
                        text_button("new-claude", "✳ Claude")
                            .on_click(cx.listener(|this, _, window, cx| this.new_claude_tab(None, window, cx))),
                    ),
            )
            .child(div().flex_1())
            .child(
                div().px_2().child(
                    text_button("history-button", "Historique")
                        .when(self.claude.history.is_some(), |b| b.bg(theme::selected()).text_color(theme::text()))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_history(window, cx))),
                ),
            )
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

fn text_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(22.0))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .text_sm()
        .text_color(theme::text_muted())
        .hover(|s| s.bg(theme::selected()).text_color(theme::text()))
        .cursor_pointer()
        .child(label)
}
