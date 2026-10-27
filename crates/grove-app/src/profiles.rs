//! Profils de terminal côté interface (specs §11.1) : catalogue, sélecteur à
//! côté du `+`, onglets SSH (connexion différée au redémarrage, bandeau quand
//! la connexion se ferme).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{Context, FocusHandle, KeyDownEvent, SharedString, Window, div, prelude::*, px, uniform_list};
use grove_core::{Profile, ProfileKind, ProfileSource, ProjectConfig, ProjectId, Reconnect, ResolvedProfile, TabId, TabKind};
use grove_term::TerminalOptions;

use crate::grove_view::GroveView;
use crate::list::{ListScroll, ROW_HEIGHT};
use crate::text_field::{self, Edit, TextField};
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, hint, icon, icon_button, kbd, list_row, popover, popover_footer, search_field, sized_icon};

/// Profils lus sur disque : ceux des projets (`.grove.toml`) et les hôtes de
/// `~/.ssh/config`. Relus à l'ouverture du sélecteur et des Paramètres.
#[derive(Default)]
pub(crate) struct ProfileCatalog {
    project: HashMap<ProjectId, (PathBuf, Vec<Profile>)>,
    ssh_hosts: Vec<String>,
}

impl ProfileCatalog {
    pub fn reload_project(&mut self, id: ProjectId, root: &Path) {
        let file = root.join(grove_core::project_config::FILE_NAME);
        let profiles = ProjectConfig::load(root)
            .map_err(|err| log::warn!("{err:#}"))
            .map(|config| config.terminal.profiles)
            .unwrap_or_default();
        self.project.insert(id, (file, profiles));
    }

    pub fn reload_ssh(&mut self) {
        self.ssh_hosts = std::env::var_os("HOME")
            .map(|home| grove_core::profile::ssh_hosts(&PathBuf::from(home).join(".ssh/config")))
            .unwrap_or_default();
    }
}

/// Onglet SSH sans processus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Offline {
    /// Restauré au démarrage : la connexion attend un clic.
    NotConnected,
    /// La connexion s'est fermée (code de sortie de `ssh`) ; le terminal reste affiché.
    Closed(Option<i32>),
}

pub(crate) struct ProfilePicker {
    project: ProjectId,
    /// `None` en tête : le shell.
    profiles: Vec<Option<ResolvedProfile>>,
    query: TextField,
    selected: usize,
    scroll: ListScroll,
    pub focus: FocusHandle,
}

impl ProfilePicker {
    fn items(&self) -> Vec<Option<ResolvedProfile>> {
        let query = self.query.as_str().trim().to_lowercase();
        self.profiles
            .iter()
            .filter(|p| {
                query.is_empty()
                    || match p {
                        Some(p) => p.profile.name.to_lowercase().contains(&query) || detail(&p.profile).to_lowercase().contains(&query),
                        None => "terminal shell".contains(&query),
                    }
            })
            .cloned()
            .collect()
    }
}

/// Ce que lance le profil, en une ligne : `deploy@staging-web-1`, `psql -U app`.
pub(crate) fn detail(profile: &Profile) -> String {
    match profile.kind() {
        ProfileKind::Shell => "shell".into(),
        ProfileKind::Command => profile.command.clone().unwrap_or_default(),
        ProfileKind::Ssh => {
            let host = profile.host.clone().unwrap_or_else(|| profile.name.clone());
            let host = match &profile.user {
                Some(user) if !user.is_empty() => format!("{user}@{host}"),
                _ => host,
            };
            match profile.port {
                Some(port) => format!("{host}:{port}"),
                None => host,
            }
        }
    }
}

pub(crate) fn source_label(source: &ProfileSource) -> &'static str {
    match source {
        ProfileSource::Project(_) => "projet",
        ProfileSource::Global => "",
        ProfileSource::SshConfig => "~/.ssh/config",
    }
}

pub(crate) fn profile_icon(profile: &Profile) -> Icon {
    match profile.kind() {
        ProfileKind::Ssh => Icon::Server,
        ProfileKind::Shell | ProfileKind::Command => Icon::Terminal,
    }
}

impl GroveView {
    /// Profils proposés dans le projet `project`, du plus prioritaire au moins prioritaire.
    pub(crate) fn resolved_profiles(&self, project: ProjectId) -> Vec<ResolvedProfile> {
        let (file, local) = self.profiles.project.get(&project).map(|(f, p)| (f.as_path(), p.as_slice())).unwrap_or((Path::new(""), &[]));
        grove_core::profile::resolve(local, file, &self.config.terminal.profiles, &self.profiles.ssh_hosts)
    }

    pub(crate) fn find_profile(&self, project: ProjectId, name: &str) -> Option<ResolvedProfile> {
        self.resolved_profiles(project).into_iter().find(|p| p.profile.name == name)
    }

    /// Profil lancé par l'onglet `tab`, s'il en a un et qu'il existe encore.
    pub(crate) fn tab_profile(&self, tab: TabId) -> Option<ResolvedProfile> {
        let key = self.locate_tab(tab)?;
        let TabKind::Terminal { profile: Some(name) } = &self.state.workspace(key)?.tabs.iter().find(|t| t.id == tab)?.kind else {
            return None;
        };
        self.find_profile(key.0, name)
    }

    pub(crate) fn is_ssh_tab(&self, tab: TabId) -> bool {
        self.tab_profile(tab).is_some_and(|p| p.profile.kind() == ProfileKind::Ssh)
    }

    /// Options du PTY d'un onglet de `profile`, ouvert dans l'espace `workspace`.
    pub(crate) fn profile_options(&self, profile: &Profile, workspace: &Path) -> TerminalOptions {
        let mut options = self.terminal_options(profile.start_dir(workspace));
        let launch = profile.launch(&self.config.shell());
        options.program = launch.program;
        options.args = launch.args;
        options.env.extend(profile.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        options
    }

    /// Onglets SSH restaurés : pas de connexion automatique, sauf `reconnect = "auto"`.
    pub(crate) fn mark_restored_ssh_tabs(&mut self) {
        let tabs: Vec<_> = self
            .state
            .projects
            .iter()
            .flat_map(|p| p.workspaces.iter().flat_map(|w| &w.tabs))
            .map(|t| t.id)
            .collect();
        for tab in tabs {
            if self.tab_profile(tab).is_some_and(|p| p.profile.kind() == ProfileKind::Ssh && p.profile.reconnect == Reconnect::Manual) {
                self.offline.insert(tab, Offline::NotConnected);
            }
        }
    }

    /// Ouvre un onglet du profil `name` (`None` : le shell) dans l'espace actif.
    pub(crate) fn open_profile_tab(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.state.active_workspace else { return };
        if let Some(workspace) = self.state.workspace_mut(key) {
            workspace.add_profile_tab(name);
            self.save();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// (Re)lance le processus d'un onglet SSH.
    pub(crate) fn connect_tab(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.offline.remove(&tab);
        self.terminals.remove(&tab);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn open_profile_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_picker.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let Some((project, _)) = self.state.active_workspace else { return };
        if let Some(root) = self.state.project(project).map(|p| p.root.clone()) {
            self.profiles.reload_project(project, &root);
        }
        self.profiles.reload_ssh();
        let profiles = std::iter::once(None).chain(self.resolved_profiles(project).into_iter().map(Some)).collect();
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.profile_picker = Some(ProfilePicker {
            project,
            profiles,
            query: TextField::default(),
            selected: 0,
            scroll: ListScroll::default(),
            focus,
        });
        cx.notify();
    }

    fn close_profile_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_picker = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn pick_profile(&mut self, profile: Option<ResolvedProfile>, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_picker = None;
        self.open_profile_tab(profile.map(|p| p.profile.name), window, cx);
    }

    pub(crate) fn profile_picker_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.profile_picker else { return };
        let count = picker.items().len();
        match event.keystroke.key.as_str() {
            "escape" => return self.close_profile_picker(window, cx),
            "enter" => {
                if let Some(item) = picker.items().get(picker.selected).cloned() {
                    return self.pick_profile(item, window, cx);
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
            _ => match picker.query.key_down(&event.keystroke, cx) {
                Edit::Ignored => return,
                Edit::Changed | Edit::Copy { cut: true, .. } => {
                    picker.selected = 0;
                    picker.scroll.reset();
                }
                Edit::Selection | Edit::Copy { .. } => {}
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn profile_row(&self, index: usize, item: &Option<ResolvedProfile>, selected: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (glyph, name, detail, source) = match item {
            Some(p) => (profile_icon(&p.profile), p.profile.name.clone(), detail(&p.profile), source_label(&p.source)),
            None => (Icon::Terminal, "Terminal".to_owned(), self.config.shell(), ""),
        };
        list_row(format!("profile-item-{index}"), selected)
            .h(ROW_HEIGHT)
            .on_click(cx.listener({
                let item = item.clone();
                move |this, _, window, cx| this.pick_profile(item.clone(), window, cx)
            }))
            .text_color(theme::text())
            .child(icon(glyph, if selected { theme::accent() } else { theme::text_faint() }))
            .child(div().flex_none().child(name))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .child(detail),
            )
            .when(!source.is_empty(), |d| d.child(div().flex_none().text_xs().text_color(theme::text_faint()).child(source)))
            .into_any_element()
    }

    pub(crate) fn render_profile_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let picker = self.profile_picker.as_ref()?;
        let count = picker.items().len();
        let list = div()
            .relative()
            .h(ListScroll::height(count))
            .child(
                uniform_list(
                    "profile-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let Some(picker) = &this.profile_picker else { return Vec::new() };
                        let (items, selected) = (picker.items(), picker.selected);
                        range.filter_map(|i| Some(this.profile_row(i, items.get(i)?, i == selected, cx))).collect()
                    }),
                )
                .track_scroll(picker.scroll.handle.clone())
                .size_full(),
            )
            .children(picker.scroll.scrollbar(count));
        let project = picker.project;
        let panel = popover("profile-picker", &picker.focus, px(440.0))
            .on_key_down(cx.listener(Self::profile_picker_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.popover_dismissed = true;
                this.close_profile_picker(window, cx);
            }))
            .child(search_field(&picker.query, "Ouvrir un terminal avec le profil…"))
            .child(text_field::input_target(&picker.focus, cx.entity()))
            .child(div().p_1().child(list))
            .when(count == 0, |d| d.child(div().px_3().py_2().text_color(theme::text_muted()).child("Aucun profil.")))
            .child(
                popover_footer()
                    .child(hint("↑↓", "naviguer"))
                    .child(hint("Entrée", "ouvrir"))
                    .child(hint("Échap", "fermer"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("manage-profiles")
                            .text_color(theme::link())
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.profile_picker = None;
                                this.open_settings(Some(crate::settings::Section::Profiles), Some(project), window, cx);
                            }))
                            .child("Gérer les profils…"),
                    ),
            );
        Some(picker.scroll.track_drag(panel, count).into_any_element())
    }

    /// Onglet SSH restauré, pas encore connecté : à la place du terminal.
    pub(crate) fn render_not_connected(&self, tab: TabId, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.offline.get(&tab) != Some(&Offline::NotConnected) {
            return None;
        }
        let profile = self.tab_profile(tab)?.profile;
        Some(
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
                        .child(sized_icon(Icon::Server, px(20.0), theme::text_muted())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(profile.name.clone()))
                        .child(div().text_sm().text_color(theme::text_muted()).child(detail(&profile))),
                )
                .child(
                    button("ssh-connect", "Se connecter", ButtonStyle::Primary)
                        .child(sized_icon(Icon::Plug, px(13.0), theme::on_accent()))
                        .on_click(cx.listener(move |this, _, window, cx| this.connect_tab(tab, window, cx))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child("Restauré sans connexion ·")
                        .child(kbd("Entrée"))
                        .child("pour se connecter"),
                )
                .into_any_element(),
        )
    }

    /// Bandeau d'un onglet SSH dont la connexion s'est fermée.
    pub(crate) fn render_connection_closed(&self, tab: TabId, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let Some(Offline::Closed(code)) = self.offline.get(&tab) else { return None };
        let text: SharedString = match code {
            Some(code) => format!("Connexion fermée (code {code})").into(),
            None => "Connexion fermée".into(),
        };
        let color = if code.is_some_and(|c| c != 0) { theme::danger() } else { theme::text_muted() };
        let key = self.locate_tab(tab)?;
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .pl_3()
                .pr_2()
                .py_1p5()
                .bg(theme::panel())
                .border_b_1()
                .border_color(theme::border())
                .text_sm()
                .child(icon(Icon::Server, color))
                .child(div().flex_1().min_w_0().text_color(theme::text()).child(text))
                .child(
                    button("ssh-reconnect", "Reconnecter", ButtonStyle::Primary)
                        .on_click(cx.listener(move |this, _, window, cx| this.connect_tab(tab, window, cx))),
                )
                .child(
                    icon_button("ssh-close-tab", Icon::X)
                        .on_click(cx.listener(move |this, _, window, cx| this.close_tab(key, tab, window, cx))),
                )
                .into_any_element(),
        )
    }
}
