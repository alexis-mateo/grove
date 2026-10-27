//! Volets d'un onglet (splits) : rendu de la disposition, séparateurs
//! déplaçables, partage, fermeture et navigation entre volets.

use std::time::Duration;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, DragMoveEvent, Render, SharedString, Window, div,
    prelude::*, px, relative,
};
use grove_core::{Axis, ClosedPane, Direction, Layout, TabId, TabKind};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::theme;

/// Délai avant d'enregistrer une proportion après le dernier mouvement du séparateur.
const SAVE_DELAY: Duration = Duration::from_millis(300);
/// Zone saisissable de part et d'autre d'un séparateur.
const HANDLE: f32 = 7.0;

/// Séparateur en cours de glisser.
#[derive(Clone)]
pub(crate) struct DraggedDivider {
    tab: TabId,
    path: Vec<usize>,
}

impl Render for DraggedDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl GroveView {
    /// Onglet actif et son volet actif (le principal s'il n'est pas partagé).
    pub(crate) fn active_pane(&self) -> Option<(WorkspaceKey, TabId, TabId)> {
        let (key, tab) = self.active_tab()?;
        let owner = self.state.workspace(key)?.tabs.iter().find(|t| t.id == tab)?;
        let pane = self.focused_pane.get(&tab).copied().filter(|p| owner.has_pane(*p)).unwrap_or(tab);
        Some((key, tab, pane))
    }

    /// Partage le volet actif ; le nouveau, à droite (`Row`) ou dessous, prend le focus.
    pub(crate) fn split_active(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, tab, pane)) = self.active_pane() else { return };
        // Même profil que le volet partagé si c'est un terminal ; sinon le profil par défaut.
        let profile = match self.state.workspace(key).and_then(|w| w.pane_owner(pane)).and_then(|t| t.pane_kind(pane)) {
            Some(TabKind::Terminal { profile }) => profile.clone(),
            _ => self.config.terminal.default_profile.clone().filter(|name| self.find_profile(key.0, name).is_some()),
        };
        let Some(owner) = self.state.workspace_mut(key).and_then(|w| w.tabs.iter_mut().find(|t| t.id == tab)) else { return };
        let Some(new) = owner.split(pane, axis, TabKind::Terminal { profile }) else { return };
        self.focused_pane.insert(tab, new);
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Ferme un volet, ou l'onglet s'il n'en a qu'un. Si le volet principal part,
    /// le suivant le remplace et reprend l'identifiant de l'onglet.
    pub(crate) fn close_pane(&mut self, pane: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.locate_tab(pane) else { return };
        let Some(owner) = self.state.workspace_mut(key).and_then(|w| w.pane_owner_mut(pane)) else { return };
        let tab = owner.id;
        match owner.close_pane(pane) {
            None => return self.close_tab(key, tab, window, cx),
            Some(ClosedPane::Removed) => self.drop_pane(pane),
            Some(ClosedPane::Promoted { pane: promoted }) => {
                self.drop_pane(tab);
                if let Some(terminal) = self.terminals.remove(&promoted) {
                    self.terminals.insert(tab, terminal);
                }
                if let Some(offline) = self.offline.remove(&promoted) {
                    self.offline.insert(tab, offline);
                }
                if self.focused_pane.get(&tab) == Some(&promoted) {
                    self.focused_pane.insert(tab, tab);
                }
            }
        }
        if self.focused_pane.get(&tab) == Some(&pane) {
            self.focused_pane.remove(&tab);
        }
        self.save();
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Oublie les processus et l'état d'un volet fermé.
    pub(crate) fn drop_pane(&mut self, pane: TabId) {
        self.terminals.remove(&pane);
        self.natives.remove(&pane);
        self.offline.remove(&pane);
        self.claude.sessions.remove(&pane);
        self.pending_commands.remove(&pane);
    }

    /// Passe au volet voisin ; `false` s'il n'y en a pas (la touche va alors au programme).
    pub(crate) fn focus_neighbor(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((key, tab, pane)) = self.active_pane() else { return false };
        let next = self.state.workspace(key).and_then(|w| w.tabs.iter().find(|t| t.id == tab)).and_then(|t| t.neighbor(pane, direction));
        let Some(next) = next else { return false };
        self.focused_pane.insert(tab, next);
        self.focus_active(window, cx);
        cx.notify();
        true
    }

    fn set_split_ratio(&mut self, key: WorkspaceKey, tab: TabId, path: &[usize], ratio: f32, cx: &mut Context<Self>) {
        let Some(owner) = self.state.workspace_mut(key).and_then(|w| w.tabs.iter_mut().find(|t| t.id == tab)) else { return };
        if owner.set_ratio(path, ratio) {
            self.layout_save = Some(cx.spawn(async move |this, cx| {
                gpui::Timer::after(SAVE_DELAY).await;
                let _ = this.update(cx, |this, _| this.save());
            }));
            cx.notify();
        }
    }

    /// Contenu de l'onglet `tab` : son volet, ou la disposition de ses volets.
    pub(crate) fn render_panes(&mut self, key: WorkspaceKey, tab: TabId, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let layout = self.state.workspace(key)?.tabs.iter().find(|t| t.id == tab)?.layout.clone();
        match layout {
            Some(layout) => Some(self.render_layout(key, tab, &layout, Vec::new(), window, cx)),
            None => self.render_pane(key, tab, false, window, cx),
        }
    }

    fn render_layout(
        &mut self,
        key: WorkspaceKey,
        tab: TabId,
        layout: &Layout,
        path: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layout::Split { axis, ratio, first, second } = layout else {
            let Layout::Pane { id } = layout else { unreachable!() };
            // Volet qui n'a pas pu démarrer : une zone vide plutôt qu'un trou dans la disposition.
            return self.render_pane(key, *id, true, window, cx).unwrap_or_else(|| div().size_full().into_any_element());
        };
        let row = *axis == Axis::Row;
        let child_path = |index| path.iter().copied().chain([index]).collect::<Vec<_>>();
        let first = self.render_layout(key, tab, first, child_path(0), window, cx);
        let second = self.render_layout(key, tab, second, child_path(1), window, cx);
        let id = SharedString::from(format!("split-{tab}-{path:?}"));

        let handle = div()
            .id(SharedString::from(format!("divider-{tab}-{path:?}")))
            .absolute()
            .when(row, |d| d.top_0().bottom_0().left(px(-HANDLE / 2.0)).w(px(HANDLE)).cursor(CursorStyle::ResizeLeftRight))
            .when(!row, |d| d.left_0().right_0().top(px(-HANDLE / 2.0)).h(px(HANDLE)).cursor(CursorStyle::ResizeUpDown))
            .on_drag(DraggedDivider { tab, path: path.clone() }, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dragged.clone())
            })
            // Double-clic : moitié-moitié.
            .on_click(cx.listener({
                let path = path.clone();
                move |this, event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        this.set_split_ratio(key, tab, &path, 0.5, cx);
                    }
                }
            }));
        let divider = div()
            .flex_none()
            .relative()
            .bg(theme::border_strong())
            .when(row, |d| d.w(px(1.0)).h_full())
            .when(!row, |d| d.h(px(1.0)).w_full())
            .child(handle);

        div()
            .id(id)
            .size_full()
            .flex()
            .when(!row, |d| d.flex_col())
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedDivider>, _, cx| {
                let dragged = event.drag(cx);
                if dragged.tab != tab || dragged.path != path {
                    return;
                }
                let (bounds, position) = (event.bounds, event.event.position);
                let ratio = if row {
                    (position.x - bounds.left()) / bounds.size.width
                } else {
                    (position.y - bounds.top()) / bounds.size.height
                };
                this.set_split_ratio(key, tab, &path.clone(), ratio, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .min_w_0()
                    .min_h_0()
                    .when(row, |d| d.w(relative(*ratio)).h_full())
                    .when(!row, |d| d.h(relative(*ratio)).w_full())
                    .child(first),
            )
            .child(divider)
            .child(div().flex_1().min_w_0().min_h_0().child(second))
            .into_any_element()
    }

    /// Un volet : son terminal (ou la vue native), avec le bandeau de connexion
    /// fermée ; un onglet SSH restauré montre à la place le bouton « Se connecter ».
    fn render_pane(&mut self, key: WorkspaceKey, pane: TabId, split: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let content = match self.render_not_connected(pane, cx) {
            Some(panel) => panel,
            None => {
                let view = self.pane_view(key, pane, window, cx)?;
                // Vue mise en cache : elle n'est redessinée que si le terminal
                // change, pas à chaque survol ou défilement ailleurs dans la fenêtre.
                view.cached(gpui::StyleRefinement::default().size_full()).into_any_element()
            }
        };
        let focused = split && self.active_pane().is_some_and(|(_, _, p)| p == pane);
        let tab = self.locate_tab(pane).and_then(|key| self.state.workspace(key)?.pane_owner(pane).map(|t| t.id));
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .when(split, |d| {
                    d.border_1()
                        .border_color(if focused { theme::accent().opacity(0.45) } else { gpui::transparent_black() })
                        // Avant le terminal, qui peut garder le clic pour lui (sélection, souris du programme).
                        .capture_any_mouse_down(cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                            if let Some(tab) = tab
                                && this.focused_pane.get(&tab) != Some(&pane)
                            {
                                this.focused_pane.insert(tab, pane);
                                cx.notify();
                            }
                        }))
                })
                .children(self.render_connection_closed(pane, cx))
                .child(div().flex_1().min_h_0().child(content))
                .into_any_element(),
        )
    }
}
