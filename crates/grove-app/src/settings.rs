//! Page Paramètres (specs §11.2). Elle remplace la zone des onglets, la sidebar
//! reste visible. `config.toml` reste la source de vérité : chaque réglage y est
//! écrit (commentaires et mise en forme gardés) puis relu ; le fichier est
//! surveillé, une modification à la main s'applique sans redémarrer.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use gpui::{AnyElement, Context, FocusHandle, KeyDownEvent, Pixels, SharedString, Task, Window, div, prelude::*, px};
use grove_core::{ClaudeView, Config, ConfigFile, Profile, ProfileKind, ProfileSource, ProjectId, Reconnect, ResolvedProfile, TabKind};

use crate::grove_view::{GroveView, ToggleSidebar};
use crate::profiles::{detail, profile_icon, source_label};
use crate::shortcuts;
use crate::text_field::{self, Edit, TextField};
use crate::theme;
use crate::ui::{ButtonStyle, Icon, button, icon, icon_button, kbd, sized_icon, tooltip, vertical_divider};
use crate::window_frame::{TOP_BAR_HEIGHT, drag_region, window_controls};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Terminal,
    Profiles,
    Claude,
    GitHub,
    Shortcuts,
}

const SECTIONS: [(Section, &str, Icon); 6] = [
    (Section::General, "Général", Icon::Settings),
    (Section::Terminal, "Terminal", Icon::Terminal),
    (Section::Profiles, "Profils", Icon::Server),
    (Section::Claude, "Claude", Icon::Claude),
    (Section::GitHub, "GitHub", Icon::GitPullRequest),
    (Section::Shortcuts, "Raccourcis clavier", Icon::Keyboard),
];

/// Champ de texte de la page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldId {
    FontFamily,
    FontSize,
    Shell,
    Scrollback,
    ClaudeCommand,
    GitHubToken,
    ReviewPrompt,
    AddressPrompt,
    Profile(ProfileField),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProfileField {
    Name,
    Command,
    Cwd,
    Env,
    Host,
    User,
    Port,
    Identity,
    Jump,
    Options,
    RemoteCommand,
}

pub(crate) struct Field {
    text: TextField,
    focus: FocusHandle,
}

/// Profil affiché dans la section Profils.
struct ProfileEditor {
    /// Nom sous lequel il est enregistré dans `config.toml` ; `None` : nouveau.
    saved: Option<String>,
    source: ProfileSource,
    draft: Profile,
}

impl ProfileEditor {
    fn editable(&self) -> bool {
        self.source == ProfileSource::Global
    }
}

enum Message {
    Info(String),
    Error(String),
}

pub(crate) struct Settings {
    section: Section,
    /// Projet dont on montre les profils (`.grove.toml`) : l'espace actif à l'ouverture.
    project: Option<ProjectId>,
    pub focus: FocusHandle,
    /// Champ en cours de saisie ; validé par Entrée ou en passant à un autre.
    editing: Option<(FieldId, Field)>,
    /// Recherche de la section Raccourcis.
    search: Field,
    editor: Option<ProfileEditor>,
    /// Liste déroulante du profil par défaut ouverte.
    default_open: bool,
    message: Option<Message>,
}

/// Surveillance de `config.toml`.
pub(crate) struct ConfigWatch {
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
}

fn opt(text: &str) -> Option<String> {
    Some(text.trim().to_owned()).filter(|t| !t.is_empty())
}

/// `A=1, B=deux` → variables d'environnement.
fn parse_env(text: &str) -> Result<std::collections::BTreeMap<String, String>> {
    let mut env = std::collections::BTreeMap::new();
    for pair in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else { bail!("« {pair} » : il manque « = »") };
        env.insert(key.trim().to_owned(), value.trim().to_owned());
    }
    Ok(env)
}

impl ProfileField {
    fn get(self, profile: &Profile) -> String {
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        match self {
            Self::Name => profile.name.clone(),
            Self::Command => text(&profile.command),
            Self::Cwd => text(&profile.cwd),
            Self::Env => profile.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", "),
            Self::Host => text(&profile.host),
            Self::User => text(&profile.user),
            Self::Port => profile.port.map(|p| p.to_string()).unwrap_or_default(),
            Self::Identity => text(&profile.identity),
            Self::Jump => text(&profile.jump),
            Self::Options => profile.options.join(", "),
            Self::RemoteCommand => text(&profile.remote_command),
        }
    }

    fn set(self, profile: &mut Profile, text: &str) -> Result<()> {
        match self {
            Self::Name => profile.name = text.trim().to_owned(),
            Self::Command => profile.command = opt(text),
            Self::Cwd => profile.cwd = opt(text),
            Self::Env => profile.env = parse_env(text)?,
            Self::Host => profile.host = opt(text),
            Self::User => profile.user = opt(text),
            Self::Port => {
                profile.port = match opt(text) {
                    Some(port) => Some(port.parse().with_context(|| format!("port invalide : {port}"))?),
                    None => None,
                }
            }
            Self::Identity => profile.identity = opt(text),
            Self::Jump => profile.jump = opt(text),
            Self::Options => profile.options = text.split(',').filter_map(opt).collect(),
            Self::RemoteCommand => profile.remote_command = opt(text),
        }
        Ok(())
    }
}

impl GroveView {
    pub(crate) fn open_settings(
        &mut self,
        section: Option<Section>,
        project: Option<ProjectId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = project.or(self.state.active_workspace.map(|(p, _)| p));
        if let Some(root) = project.and_then(|p| self.state.project(p)).map(|p| p.root.clone()) {
            self.profiles.reload_project(project.expect("projet"), &root);
        }
        self.profiles.reload_ssh();
        self.branch_picker = None;
        self.profile_picker = None;
        self.rename = None;
        let section = section.or(self.settings.as_ref().map(|s| s.section)).unwrap_or(Section::General);
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.settings = Some(Settings {
            section,
            project,
            focus,
            editing: None,
            search: Field { text: TextField::default(), focus: cx.focus_handle() },
            editor: None,
            default_open: false,
            message: None,
        });
        self.select_section(section, window, cx);
    }

    /// Ferme la page (Échap) et rend le focus à l'onglet actif.
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_settings(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Quitte la page sans perdre la saisie en cours (navigation vers un onglet).
    pub(crate) fn leave_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.commit_edit(cx);
            self.settings = None;
        }
    }

    fn select_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let Some(settings) = &mut self.settings else { return };
        settings.section = section;
        settings.default_open = false;
        settings.message = None;
        if section == Section::Shortcuts {
            window.focus(&settings.search.focus);
        } else {
            window.focus(&settings.focus);
        }
        if section == Section::Profiles && settings.editor.is_none() {
            let first = self.config.terminal.profiles.first().cloned();
            if let (Some(profile), Some(settings)) = (first, &mut self.settings) {
                settings.editor = Some(ProfileEditor { saved: Some(profile.name.clone()), source: ProfileSource::Global, draft: profile });
            }
        }
        cx.notify();
    }

    fn field_value(&self, id: FieldId) -> String {
        let config = &self.config;
        match id {
            FieldId::FontFamily => config.font.family.clone(),
            FieldId::FontSize => format!("{}", config.font.size),
            FieldId::Shell => config.terminal.shell.clone().unwrap_or_default(),
            FieldId::Scrollback => config.terminal.scrollback.to_string(),
            FieldId::ClaudeCommand => config.claude.command.clone(),
            // Jamais réaffiché : la saisie part d'un champ vide.
            FieldId::GitHubToken => String::new(),
            FieldId::ReviewPrompt => config.github.review.prompt.clone(),
            FieldId::AddressPrompt => config.github.address.prompt.clone(),
            FieldId::Profile(field) => {
                self.settings.as_ref().and_then(|s| s.editor.as_ref()).map(|e| field.get(&e.draft)).unwrap_or_default()
            }
        }
    }

    fn start_edit(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.as_ref().and_then(|s| s.editing.as_ref()).is_some_and(|(f, _)| *f == id) {
            return;
        }
        self.commit_edit(cx);
        let text = TextField::selected(self.field_value(id));
        let Some(settings) = &mut self.settings else { return };
        let focus = cx.focus_handle();
        window.focus(&focus);
        settings.editing = Some((id, Field { text, focus }));
        settings.default_open = false;
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = &mut self.settings {
            settings.editing = None;
            window.focus(&settings.focus);
        }
        cx.notify();
    }

    /// Valide le champ en cours : écrit `config.toml`, ou le brouillon du profil.
    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, field)) = self.settings.as_mut().and_then(|s| s.editing.take()) else { return };
        let text = field.text.as_str().to_owned();
        if text == self.field_value(id) {
            return;
        }
        let result = match id {
            FieldId::Profile(field) => match self.settings.as_mut().and_then(|s| s.editor.as_mut()) {
                Some(editor) => field.set(&mut editor.draft, &text),
                None => Ok(()),
            },
            FieldId::FontFamily => self.write_config(cx, |file| {
                match opt(&text) {
                    Some(family) => file.set(&["font", "family"], family),
                    None => file.remove(&["font", "family"]),
                }
                Ok(())
            }),
            FieldId::FontSize => match text.trim().replace(',', ".").parse::<f64>() {
                Ok(size) if (6.0..=48.0).contains(&size) => self.write_config(cx, |file| { file.set(&["font", "size"], size); Ok(()) }),
                _ => Err(anyhow::anyhow!("taille de police invalide : « {text} » (de 6 à 48)")),
            },
            FieldId::Shell => self.write_config(cx, |file| {
                match opt(&text) {
                    Some(shell) => file.set(&["terminal", "shell"], shell),
                    None => file.remove(&["terminal", "shell"]),
                }
                Ok(())
            }),
            FieldId::Scrollback => match text.trim().replace([' ', '\u{202f}'], "").parse::<i64>() {
                Ok(lines) if (0..=1_000_000).contains(&lines) => {
                    self.write_config(cx, |file| { file.set(&["terminal", "scrollback"], lines); Ok(()) })
                }
                _ => Err(anyhow::anyhow!("scrollback invalide : « {text} » (de 0 à 1 000 000 lignes)")),
            },
            FieldId::ClaudeCommand => self.write_config(cx, |file| {
                match opt(&text) {
                    Some(command) => file.set(&["claude", "command"], command),
                    None => file.remove(&["claude", "command"]),
                }
                Ok(())
            }),
            FieldId::GitHubToken => self.set_github_token(Some(&text), cx).map(|()| {
                if let Some(settings) = &mut self.settings {
                    settings.message = Some(Message::Info("Jeton GitHub enregistré.".into()));
                }
            }),
            FieldId::ReviewPrompt | FieldId::AddressPrompt => {
                let table = if id == FieldId::ReviewPrompt { "review" } else { "address" };
                self.write_config(cx, |file| {
                    // Ancien nom de la clé : il masquerait la nouvelle valeur.
                    file.remove(&["github", table, "skill"]);
                    match opt(&text) {
                        Some(prompt) => file.set(&["github", table, "prompt"], prompt),
                        None => file.remove(&["github", table, "prompt"]),
                    }
                    Ok(())
                })
            }
        };
        if let (Err(err), Some(settings)) = (result, &mut self.settings) {
            settings.message = Some(Message::Error(format!("{err:#}")));
        }
        cx.notify();
    }

    /// Modifie `config.toml` puis applique la configuration relue.
    fn write_config(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut ConfigFile) -> Result<()>) -> Result<()> {
        let path = self.paths.config_file();
        let mut file = ConfigFile::open(&path)?;
        edit(&mut file)?;
        file.save()?;
        let config = Config::load(&path)?;
        self.apply_config(config, cx);
        if let Some(settings) = &mut self.settings {
            settings.message = None;
        }
        Ok(())
    }

    fn write_setting(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut ConfigFile) -> Result<()>) {
        if let Err(err) = self.write_config(cx, edit)
            && let Some(settings) = &mut self.settings
        {
            settings.message = Some(Message::Error(format!("{err:#}")));
        }
        cx.notify();
    }

    /// Applique une configuration relue : police des terminaux ouverts, API
    /// d'usage ; le reste vaut pour les prochains onglets.
    pub(crate) fn apply_config(&mut self, config: Config, cx: &mut Context<Self>) {
        if config == self.config {
            return;
        }
        if config.font != self.config.font {
            for terminal in self.terminals.values() {
                terminal.update(cx, |terminal, cx| terminal.set_font(&config.font.family, config.font.size, cx));
            }
        }
        if config.claude.usage_api != self.config.claude.usage_api {
            self.claude.set_usage_api(config.claude.usage_api, cx);
        }
        self.config = config;
        cx.notify();
    }

    /// Surveille `config.toml` : une modification à la main s'applique aussitôt.
    pub(crate) fn watch_config(&mut self, cx: &mut Context<Self>) {
        use notify::Watcher;

        let path = self.paths.config_file();
        let Some(dir) = path.parent().map(Path::to_owned) else { return };
        if let Err(err) = std::fs::create_dir_all(&dir) {
            log::warn!("création de {} : {err}", dir.display());
            return;
        }
        let (tx, rx) = smol::channel::unbounded::<()>();
        let name = path.file_name().map(|n| n.to_owned());
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && !event.kind.is_access()
                && event.paths.iter().any(|p| p.file_name() == name.as_deref())
            {
                let _ = tx.try_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(err) => return log::warn!("surveillance de {} : {err}", path.display()),
        };
        if let Err(err) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            return log::warn!("surveillance de {} : {err}", dir.display());
        }
        let task = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Un éditeur écrit en plusieurs fois : on attend la fin de la rafale.
                gpui::Timer::after(Duration::from_millis(150)).await;
                while rx.try_recv().is_ok() {}
                let config = Config::load(&path);
                let alive = this.update(cx, |this, cx| match config {
                    Ok(config) => this.apply_config(config, cx),
                    Err(err) => {
                        log::warn!("{err:#}");
                        if let Some(settings) = &mut this.settings {
                            settings.message = Some(Message::Error(format!("config.toml ignoré : {err:#}")));
                            cx.notify();
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.config_watch = Some(ConfigWatch { _watcher: watcher, _task: task });
    }

    fn open_config_file(&mut self, cx: &mut Context<Self>) {
        let path = self.paths.config_file();
        if !path.exists() {
            let header = "# Configuration de Grove. Les valeurs absentes gardent leur défaut.\n";
            if let Err(err) = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).and_then(|()| std::fs::write(&path, header)) {
                if let Some(settings) = &mut self.settings {
                    settings.message = Some(Message::Error(format!("création de {} : {err}", path.display())));
                }
                return cx.notify();
            }
        }
        cx.open_with_system(&path);
    }

    pub(crate) fn settings_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = &self.settings else { return };
        let keystroke = &event.keystroke;
        if settings.editing.as_ref().is_some_and(|(_, f)| f.focus.is_focused(window)) {
            match keystroke.key.as_str() {
                "enter" => {
                    self.commit_edit(cx);
                    if let Some(settings) = &self.settings {
                        window.focus(&settings.focus);
                    }
                }
                "escape" => self.cancel_edit(window, cx),
                _ => {
                    let Some((_, field)) = self.settings.as_mut().and_then(|s| s.editing.as_mut()) else { return };
                    if field.text.key_down(keystroke, cx) == Edit::Ignored {
                        return;
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(settings) = &mut self.settings else { return };
        if settings.search.focus.is_focused(window) && keystroke.key != "escape" {
            if settings.search.text.key_down(keystroke, cx) != Edit::Ignored {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        match keystroke.key.as_str() {
            "escape" if settings.default_open => settings.default_open = false,
            "escape" if settings.section == Section::Shortcuts && !settings.search.text.is_empty() => {
                settings.search.text = TextField::default();
            }
            "escape" => return self.close_settings(window, cx),
            "enter" if settings.section == Section::Profiles => return self.save_profile(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Champ de la page qui a le focus, pour le texte composé (é, è, ç…).
    pub(crate) fn settings_has_focus(&self, window: &Window) -> bool {
        self.settings.as_ref().is_some_and(|s| {
            s.editing.as_ref().is_some_and(|(_, f)| f.focus.is_focused(window)) || s.search.focus.is_focused(window)
        })
    }

    // Profils.

    fn edit_profile(&mut self, resolved: ResolvedProfile, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let Some(settings) = &mut self.settings else { return };
        let saved = (resolved.source == ProfileSource::Global).then(|| resolved.profile.name.clone());
        settings.editor = Some(ProfileEditor { saved, source: resolved.source, draft: resolved.profile });
        settings.message = None;
        cx.notify();
    }

    fn new_profile(&mut self, draft: Profile, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let Some(settings) = &mut self.settings else { return };
        settings.editor = Some(ProfileEditor { saved: None, source: ProfileSource::Global, draft });
        settings.message = None;
        self.start_edit(FieldId::Profile(ProfileField::Name), window, cx);
    }

    /// Nom libre dérivé de `name` : « staging (copie) », « staging (copie 2) »…
    fn free_name(&self, name: &str) -> String {
        let taken = |candidate: &str| self.config.terminal.profiles.iter().any(|p| p.name == candidate);
        let mut candidate = format!("{name} (copie)");
        let mut n = 2;
        while taken(&candidate) {
            candidate = format!("{name} (copie {n})");
            n += 1;
        }
        candidate
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let Some(editor) = self.settings.as_ref().and_then(|s| s.editor.as_ref()).filter(|e| e.editable()) else { return };
        let (saved, mut draft) = (editor.saved.clone(), editor.draft.clone());
        draft.name = draft.name.trim().to_owned();
        let error = if draft.name.is_empty() {
            Some("Le profil n'a pas de nom.".to_owned())
        } else if saved.as_deref() != Some(draft.name.as_str()) && self.config.terminal.profiles.iter().any(|p| p.name == draft.name) {
            Some(format!("Un profil « {} » existe déjà.", draft.name))
        } else if draft.kind() == ProfileKind::Command && draft.command.is_none() {
            Some("Un profil « commande » a besoin d'une commande.".to_owned())
        } else {
            None
        };
        if let Some(error) = error {
            if let Some(settings) = &mut self.settings {
                settings.message = Some(Message::Error(error));
            }
            return cx.notify();
        }
        let renamed = saved.clone().filter(|old| *old != draft.name);
        let default = self.config.terminal.default_profile.clone();
        let result = self.write_config(cx, |file| {
            file.set_profile(saved.as_deref(), &draft)?;
            if renamed.is_some() && default == renamed {
                file.set(&["terminal", "default_profile"], draft.name.as_str());
            }
            Ok(())
        });
        if let Some(old) = renamed.filter(|_| result.is_ok()) {
            self.rename_profile_tabs(&old, &draft.name);
        }
        if let Some(settings) = &mut self.settings {
            match result {
                Ok(()) => {
                    settings.message = Some(Message::Info(format!("Profil « {} » enregistré.", draft.name)));
                    settings.editor = Some(ProfileEditor { saved: Some(draft.name.clone()), source: ProfileSource::Global, draft });
                }
                Err(err) => settings.message = Some(Message::Error(format!("{err:#}"))),
            }
        }
        cx.notify();
    }

    /// Les onglets d'un profil renommé gardent leur profil.
    fn rename_profile_tabs(&mut self, old: &str, new: &str) {
        let tabs = self.state.projects.iter_mut().flat_map(|p| p.workspaces.iter_mut()).flat_map(|w| w.tabs.iter_mut());
        for tab in tabs {
            if let TabKind::Terminal { profile: Some(name) } = &mut tab.kind
                && name == old
            {
                *name = new.to_owned();
            }
        }
        self.save();
    }

    fn delete_profile(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.settings.as_ref().and_then(|s| s.editor.as_ref()).and_then(|e| e.saved.clone()) else { return };
        let was_default = self.config.terminal.default_profile.as_deref() == Some(name.as_str());
        let result = self.write_config(cx, |file| {
            file.remove_profile(&name);
            if was_default {
                file.remove(&["terminal", "default_profile"]);
            }
            Ok(())
        });
        if let Some(settings) = &mut self.settings {
            settings.editing = None;
            match result {
                Ok(()) => {
                    settings.editor = None;
                    settings.message = Some(Message::Info(format!("Profil « {name} » supprimé.")));
                }
                Err(err) => settings.message = Some(Message::Error(format!("{err:#}"))),
            }
        }
        cx.notify();
    }

    fn set_profile_kind(&mut self, kind: ProfileKind, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        if let Some(editor) = self.settings.as_mut().and_then(|s| s.editor.as_mut()) {
            editor.draft.kind = Some(kind);
        }
        cx.notify();
    }

    // Rendu.

    pub(crate) fn render_settings(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let settings = self.settings.as_ref()?;
        let header = div()
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
                    icon_button("settings-show-sidebar", Icon::PanelLeft)
                        .tooltip(tooltip("Afficher la sidebar · Ctrl+Shift+B"))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx))),
                )
            })
            .child(icon(Icon::Settings, theme::text_muted()))
            .child(div().pl_1().text_sm().font_weight(gpui::FontWeight::MEDIUM).child("Paramètres"))
            .child(drag_region(div().id("settings-drag").flex_1().h_full()))
            .child(
                button("open-config", "Ouvrir config.toml", ButtonStyle::Secondary)
                    .child(sized_icon(Icon::ExternalLink, px(12.0), theme::text_muted()))
                    .tooltip(tooltip(self.paths.config_file().display().to_string()))
                    .on_click(cx.listener(|this, _, _, cx| this.open_config_file(cx))),
            )
            .child(
                icon_button("close-settings", Icon::X)
                    .tooltip(tooltip("Fermer · Échap"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx))),
            )
            .when_some(window_controls(window), |d, controls| d.child(vertical_divider()).child(controls))
            .when(window_controls(window).is_none(), |d| d.pr_2());

        let nav = div()
            .w(px(200.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap_0p5()
            .p_2()
            .border_r_1()
            .border_color(theme::border())
            .children(SECTIONS.iter().map(|&(section, label, glyph)| {
                let selected = settings.section == section;
                div()
                    .id(SharedString::from(format!("settings-section-{label}")))
                    .h(px(30.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::selected()).text_color(theme::text()))
                    .when(!selected, |d| d.text_color(theme::text_muted()).hover(|s| s.bg(theme::hover()).text_color(theme::text())))
                    .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
                    .child(icon(glyph, if selected { theme::accent() } else { theme::text_faint() }))
                    .child(label)
            }));

        let content = match settings.section {
            Section::General => self.render_general(cx),
            Section::Terminal => self.render_terminal_settings(cx),
            Section::Profiles => self.render_profiles_settings(cx),
            Section::Claude => self.render_claude_settings(cx),
            Section::GitHub => self.render_github_settings(cx),
            Section::Shortcuts => self.render_shortcuts(cx),
        };
        let message = settings.message.as_ref().map(|message| {
            let (text, color) = match message {
                Message::Info(text) => (text.clone(), theme::accent()),
                Message::Error(text) => (text.clone(), theme::danger()),
            };
            div()
                .mb_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(color.opacity(0.4))
                .bg(color.opacity(0.08))
                .text_sm()
                .text_color(theme::text())
                .child(text)
        });

        Some(
            div()
                .id("settings")
                .track_focus(&settings.focus)
                .key_context("Settings")
                .on_key_down(cx.listener(Self::settings_key_down))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div().flex_1().min_h_0().flex().child(nav).child(
                        div()
                            .id("settings-content")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(div().max_w(px(760.0)).px_6().py_5().children(message).child(content)),
                    ),
                )
                .into_any_element(),
        )
    }

    fn render_field(&self, id: FieldId, placeholder: &'static str, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.settings.as_ref().and_then(|s| s.editing.as_ref()).filter(|(f, _)| *f == id);
        let base = div()
            .id(SharedString::from(format!("field-{id:?}")))
            .h(px(28.0))
            .w(width)
            .px_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .bg(theme::background())
            .text_sm();
        match editing {
            Some((_, field)) => base
                .track_focus(&field.focus)
                .relative()
                .border_color(theme::accent())
                .child(field.text.render(placeholder))
                .child(text_field::input_target(&field.focus, cx.entity()))
                .into_any_element(),
            None => {
                let value = self.field_value(id);
                base.border_color(theme::border_strong())
                    .cursor_text()
                    .hover(|s| s.border_color(theme::text_faint()))
                    .on_click(cx.listener(move |this, _, window, cx| this.start_edit(id, window, cx)))
                    .child(match value.is_empty() {
                        true => div().text_color(theme::text_faint()).child(placeholder),
                        false => div().whitespace_nowrap().text_ellipsis().text_color(theme::text()).child(value),
                    })
                    .into_any_element()
            }
        }
    }

    fn render_general(&self, cx: &mut Context<Self>) -> AnyElement {
        let size = self.config.font.size;
        div()
            .child(section_title("Général"))
            .child(setting_row(
                "Police",
                "Famille de la police du terminal. « monospace » : celle du système.",
                self.render_field(FieldId::FontFamily, "monospace", px(260.0), cx),
            ))
            .child(setting_row(
                "Taille",
                "En points, appliquée aux terminaux ouverts.",
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.stepper("font-smaller", "−", cx, move |file| { file.set(&["font", "size"], f64::from(size - 1.0).max(6.0)); Ok(()) }))
                    .child(self.render_field(FieldId::FontSize, "14", px(64.0), cx))
                    .child(self.stepper("font-larger", "+", cx, move |file| { file.set(&["font", "size"], f64::from(size + 1.0).min(48.0)); Ok(()) })),
            ))
            .child(setting_row("Thème", "Graphite, pour l'instant le seul.", div().text_sm().text_color(theme::text_faint()).child("Graphite")))
            .into_any_element()
    }

    fn stepper(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        edit: impl Fn(&mut ConfigFile) -> Result<()> + 'static,
    ) -> impl IntoElement {
        button(id, label, ButtonStyle::Secondary).w(px(28.0)).justify_center().on_click(cx.listener(move |this, _, _, cx| {
            this.commit_edit(cx);
            this.write_setting(cx, &edit);
        }))
    }

    fn render_terminal_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.settings.as_ref().expect("page ouverte");
        let profiles = settings.project.map(|p| self.resolved_profiles(p)).unwrap_or_default();
        let current = self.config.terminal.default_profile.clone();
        let label = current.clone().unwrap_or_else(|| "Terminal (shell)".into());
        let select = div()
            .relative()
            .child(
                div()
                    .id("default-profile")
                    .h(px(28.0))
                    .w(px(260.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::background())
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.border_color(theme::text_faint()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commit_edit(cx);
                        // Le clic qui vient de fermer la liste (clic extérieur) ne la rouvre pas.
                        let dismissed = std::mem::take(&mut this.popover_dismissed);
                        if let Some(settings) = &mut this.settings {
                            settings.default_open = !settings.default_open && !dismissed;
                        }
                        cx.notify();
                    }))
                    .child(div().flex_1().child(label))
                    .child(icon(Icon::ChevronDown, theme::text_faint())),
            )
            .when(settings.default_open, |d| {
                let options = std::iter::once(None).chain(profiles.iter().map(|p| Some(p.profile.name.clone())));
                d.child(
                    div()
                        .id("default-profile-list")
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.popover_dismissed = true;
                            if let Some(settings) = &mut this.settings {
                                settings.default_open = false;
                            }
                            cx.notify();
                        }))
                        .absolute()
                        .top(px(32.0))
                        .left_0()
                        .w(px(260.0))
                        .max_h(px(260.0))
                        .overflow_y_scroll()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::border_strong())
                        .bg(theme::elevated())
                        .shadow_lg()
                        .text_sm()
                        .children(options.enumerate().map(|(index, name)| {
                            let selected = name == current;
                            div()
                                .id(("default-profile-option", index))
                                .h(px(26.0))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::hover()))
                                .child(div().w(px(14.0)).children(selected.then(|| icon(Icon::Check, theme::accent()))))
                                .child(name.clone().unwrap_or_else(|| "Terminal (shell)".into()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(settings) = &mut this.settings {
                                        settings.default_open = false;
                                    }
                                    let name = name.clone();
                                    this.write_setting(cx, |file| {
                                        match name {
                                            Some(name) => file.set(&["terminal", "default_profile"], name),
                                            None => file.remove(&["terminal", "default_profile"]),
                                        }
                                        Ok(())
                                    });
                                }))
                        })),
                )
            });
        div()
            .child(section_title("Terminal"))
            .child(setting_row(
                "Shell",
                "Lancé dans les onglets terminal et pour les commandes des profils. Vide : $SHELL.",
                self.render_field(FieldId::Shell, "$SHELL", px(260.0), cx),
            ))
            .child(setting_row(
                "Scrollback",
                "Lignes gardées en mémoire par terminal (prochains onglets).",
                self.render_field(FieldId::Scrollback, "10000", px(120.0), cx),
            ))
            .child(setting_row("Profil par défaut", "Ouvert par Ctrl+Shift+T et le bouton +.", select))
            .into_any_element()
    }

    fn render_claude_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let claude = &self.config.claude;
        let view = claude.view;
        let views = [(ClaudeView::Terminal, "Terminal (TUI officiel)"), (ClaudeView::Native, "Vue native")];
        div()
            .child(section_title("Claude"))
            .child(setting_row(
                "Commande",
                "Lancée par le shell interactif dans les onglets Claude (alias et fonctions compris).",
                self.render_field(FieldId::ClaudeCommand, "claude", px(260.0), cx),
            ))
            .child(setting_row(
                "Vue par défaut",
                "Des nouveaux onglets Claude ; Ctrl+Shift+M bascule un onglet.",
                segmented(views.iter().map(|&(value, label)| {
                    (label, value == view, cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        let value = if value == ClaudeView::Native { "native" } else { "terminal" };
                        this.write_setting(cx, |file| { file.set(&["claude", "view"], value); Ok(()) });
                    }))
                })),
            ))
            .child(setting_row(
                "Notifications",
                "Notification système quand une session attend une réponse ou termine un tour.",
                toggle("claude-notifications", claude.notifications).on_click(cx.listener(|this, _, _, cx| {
                    let value = !this.config.claude.notifications;
                    this.write_setting(cx, |file| { file.set(&["claude", "notifications"], value); Ok(()) });
                })),
            ))
            .child(setting_row(
                "API d'usage",
                "Lit les limites 5 h et hebdomadaire avec le jeton de Claude Code, toutes les 3 min. \
                 C'est un endpoint interne d'Anthropic, hors des points d'intégration officiels : \
                 il peut changer ou disparaître sans préavis.",
                toggle("claude-usage-api", claude.usage_api).on_click(cx.listener(|this, _, _, cx| {
                    let value = !this.config.claude.usage_api;
                    this.write_setting(cx, |file| { file.set(&["claude", "usage_api"], value); Ok(()) });
                })),
            ))
            .into_any_element()
    }

    fn render_github_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let github = &self.github;
        let manual = github.token.as_ref().filter(|t| t.source == grove_github::TokenSource::Manual).map(|t| t.value.clone());
        let (status, color) = match (&github.error, &github.inbox, &github.token) {
            (Some(error), _, _) => (error.clone(), theme::danger()),
            (None, Some(inbox), Some(token)) => {
                let source = match token.source {
                    grove_github::TokenSource::GhCli => "CLI GitHub".to_owned(),
                    grove_github::TokenSource::Manual => format!("jeton saisi ({})", grove_github::mask(&token.value)),
                };
                (format!("@{} · {source}", inbox.viewer), theme::text())
            }
            _ if github.loading || !github.resolved => ("Lecture…".into(), theme::text_muted()),
            _ => ("Aucun jeton : ni saisi, ni CLI GitHub connectée.".into(), theme::waiting()),
        };
        let token_control = match manual {
            Some(token) => div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().text_color(theme::text_muted()).child(grove_github::mask(&token)))
                .child(button("github-token-remove", "Supprimer", ButtonStyle::Secondary).on_click(cx.listener(|this, _, _, cx| {
                    let result = this.set_github_token(None, cx);
                    if let Some(settings) = &mut this.settings {
                        settings.message = Some(match result {
                            Ok(()) => Message::Info("Jeton supprimé : celui de la CLI GitHub sera utilisé s'il existe.".into()),
                            Err(err) => Message::Error(format!("{err:#}")),
                        });
                    }
                    cx.notify();
                })))
                .into_any_element(),
            None => self.render_field(FieldId::GitHubToken, "ghp_… ou github_pat_…", px(260.0), cx),
        };
        div()
            .child(section_title("GitHub"))
            .child(setting_row(
                "Connexion",
                "Compte utilisé pour lire les pull requests, toutes les 2 min.",
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().max_w(px(300.0)).text_sm().text_color(color).child(status))
                    .child(
                        icon_button("github-refresh", Icon::Refresh)
                            .tooltip(tooltip("Actualiser"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.github.token = None;
                                this.refresh_github(cx);
                            })),
                    ),
            ))
            .child(setting_row(
                "Jeton d'accès",
                "Par défaut, celui de la CLI GitHub (gh auth login). Un jeton saisi ici le remplace : \
                 classique avec le scope repo, ou fine-grained avec la lecture des pull requests. \
                 Gardé dans le dossier de données de Grove, lisible par toi seul.",
                token_control,
            ))
            .child(setting_row(
                "Review",
                "Envoyé par « Reviewer ». Un skill (/review) reçoit l'URL de la PR en argument ; \
                 un prompt libre est suivi de l'URL. Par projet : [github.review] prompt dans .grove.toml.",
                self.render_field(FieldId::ReviewPrompt, "/review, ou un prompt libre", px(320.0), cx),
            ))
            .child(setting_row(
                "Commentaires reçus",
                "Envoyé par « Prendre en compte les commentaires », suivi de l'URL et des fils qui \
                 attendent ta réponse. Skill ou prompt libre, comme pour la review.",
                self.render_field(FieldId::AddressPrompt, "Analyse les commentaires de cette PR…", px(320.0), cx),
            ))
            .into_any_element()
    }

    fn render_profiles_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.settings.as_ref().expect("page ouverte");
        let resolved = settings.project.map(|p| self.resolved_profiles(p)).unwrap_or_else(|| {
            grove_core::profile::resolve(&[], Path::new(""), &self.config.terminal.profiles, &[])
        });
        let selected = settings.editor.as_ref().map(|e| (e.saved.clone().unwrap_or_else(|| e.draft.name.clone()), e.source.clone()));
        let project_name = settings.project.and_then(|p| self.state.project(p)).map(|p| p.name.clone()).unwrap_or_default();

        let mut groups: Vec<(String, Vec<ResolvedProfile>)> = Vec::new();
        let global: Vec<_> = self
            .config
            .terminal
            .profiles
            .iter()
            .map(|p| ResolvedProfile { profile: p.clone(), source: ProfileSource::Global })
            .collect();
        groups.push(("Mes profils".into(), global));
        let local: Vec<_> = resolved.iter().filter(|r| matches!(r.source, ProfileSource::Project(_))).cloned().collect();
        if !local.is_empty() {
            groups.push((format!("Projet « {project_name} »"), local));
        }
        let ssh: Vec<_> = resolved.iter().filter(|r| r.source == ProfileSource::SshConfig).cloned().collect();
        if !ssh.is_empty() {
            groups.push(("~/.ssh/config".into(), ssh));
        }

        let list = div()
            .w(px(220.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .children(groups.into_iter().enumerate().map(|(group, (title, profiles))| {
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().px_2().pb_1().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(theme::text_faint()).child(title))
                    .when(profiles.is_empty(), |d| d.child(div().px_2().text_sm().text_color(theme::text_faint()).child("Aucun.")))
                    .children(profiles.into_iter().enumerate().map(|(index, resolved)| {
                        let is_selected = selected.as_ref().is_some_and(|(name, source)| *name == resolved.profile.name && *source == resolved.source);
                        div()
                            .id(SharedString::from(format!("profile-entry-{group}-{index}")))
                            .h(px(28.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .when(is_selected, |d| d.bg(theme::selected()))
                            .when(!is_selected, |d| d.hover(|s| s.bg(theme::hover())))
                            .child(icon(profile_icon(&resolved.profile), theme::text_faint()))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(resolved.profile.name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.edit_profile(resolved.clone(), cx)))
                    }))
            }))
            .child(
                button("new-profile", "Nouveau profil", ButtonStyle::Secondary)
                    .child(sized_icon(Icon::Plus, px(12.0), theme::text_muted()))
                    .on_click(cx.listener(|this, _, window, cx| this.new_profile(Profile::default(), window, cx))),
            );

        let editor = match &settings.editor {
            Some(editor) => self.render_profile_editor(editor, cx),
            None => div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("Choisis un profil, ou crée-en un. Les hôtes de ~/.ssh/config sont proposés tels quels ; « Personnaliser » en fait un profil modifiable.")
                .into_any_element(),
        };

        div()
            .child(section_title("Profils"))
            .child(
                div()
                    .mb_4()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child("Ce qu'un onglet lance : le shell, une connexion SSH (la commande ssh du système, avec ~/.ssh/config, l'agent et vos clés) ou une commande. Grove ne stocke aucun mot de passe : ssh les demande dans le terminal."),
            )
            .child(div().flex().gap_6().child(list).child(div().flex_1().min_w_0().child(editor)))
            .into_any_element()
    }

    fn render_profile_editor(&self, editor: &ProfileEditor, cx: &mut Context<Self>) -> AnyElement {
        let draft = &editor.draft;
        let kind = draft.kind();
        if !editor.editable() {
            return self.render_readonly_profile(editor, cx);
        }
        let field = |this: &Self, f: ProfileField, placeholder: &'static str, cx: &mut Context<Self>| {
            this.render_field(FieldId::Profile(f), placeholder, px(300.0), cx)
        };
        let kinds = [(ProfileKind::Shell, "Shell"), (ProfileKind::Ssh, "SSH"), (ProfileKind::Command, "Commande")];
        let mut rows = div()
            .flex()
            .flex_col()
            .child(editor_row("Nom", field(self, ProfileField::Name, "staging", cx)))
            .child(editor_row(
                "Type",
                segmented(kinds.iter().map(|&(value, label)| {
                    (label, value == kind, cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        this.set_profile_kind(value, cx)
                    }))
                })),
            ));
        match kind {
            ProfileKind::Shell => {}
            ProfileKind::Command => {
                rows = rows.child(editor_row("Commande", field(self, ProfileField::Command, "docker compose exec db psql -U app", cx)));
            }
            ProfileKind::Ssh => {
                rows = rows
                    .child(editor_row("Hôte", field(self, ProfileField::Host, "alias de ~/.ssh/config ou nom d'hôte", cx)))
                    .child(editor_row("Utilisateur", field(self, ProfileField::User, "celui de ~/.ssh/config", cx)))
                    .child(editor_row("Port", field(self, ProfileField::Port, "22", cx)))
                    .child(editor_row("Clé (-i)", field(self, ProfileField::Identity, "~/.ssh/id_ed25519", cx)))
                    .child(editor_row("Rebond (-J)", field(self, ProfileField::Jump, "bastion", cx)))
                    .child(editor_row("Options (-o)", field(self, ProfileField::Options, "ServerAliveInterval=30, …", cx)))
                    .child(editor_row("Commande distante", field(self, ProfileField::RemoteCommand, "cd /var/www/app && exec $SHELL -l", cx)))
                    .child(editor_row(
                        "Au redémarrage",
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(toggle("profile-reconnect", draft.reconnect == Reconnect::Auto).on_click(cx.listener(|this, _, _, cx| {
                                this.commit_edit(cx);
                                if let Some(editor) = this.settings.as_mut().and_then(|s| s.editor.as_mut()) {
                                    editor.draft.reconnect = match editor.draft.reconnect {
                                        Reconnect::Auto => Reconnect::Manual,
                                        Reconnect::Manual => Reconnect::Auto,
                                    };
                                }
                                cx.notify();
                            })))
                            .child(div().text_sm().text_color(theme::text_muted()).child("se reconnecter tout seul")),
                    ));
            }
        }
        if kind != ProfileKind::Ssh {
            rows = rows.child(editor_row("Dossier de départ", field(self, ProfileField::Cwd, "l'espace de travail", cx)));
        }
        rows = rows.child(editor_row("Environnement", field(self, ProfileField::Env, "CLE=valeur, AUTRE=valeur", cx)));

        let saved = editor.saved.is_some();
        let preview = match kind {
            ProfileKind::Shell => self.config.shell(),
            _ => {
                let launch = draft.launch(&self.config.shell());
                std::iter::once(launch.program).chain(launch.args).collect::<Vec<_>>().join(" ")
            }
        };
        let actions = div()
            .flex()
            .gap_2()
            .pt_4()
            .child(button("save-profile", "Enregistrer", ButtonStyle::Primary).on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))))
            .when(saved, |d| {
                d.child(button("duplicate-profile", "Dupliquer", ButtonStyle::Secondary).on_click(cx.listener(|this, _, window, cx| {
                    let Some(mut draft) = this.settings.as_ref().and_then(|s| s.editor.as_ref()).map(|e| e.draft.clone()) else { return };
                    draft.name = this.free_name(&draft.name);
                    this.new_profile(draft, window, cx);
                })))
                .child(div().flex_1())
                .child(button("delete-profile", "Supprimer", ButtonStyle::Danger).on_click(cx.listener(|this, _, _, cx| this.delete_profile(cx))))
            });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pb_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(profile_icon(draft), theme::accent()))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(if saved { draft.name.clone() } else { "Nouveau profil".into() }))
                    .child(div().text_xs().text_color(theme::text_faint()).child("config.toml")),
            )
            .child(rows)
            .child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .child(format!("Lance : {preview}")),
            )
            .child(actions)
            .child(div().pt_2().flex().items_center().gap_1p5().text_xs().text_color(theme::text_faint()).child(kbd("Entrée")).child("valide le champ, puis enregistre"))
            .into_any_element()
    }

    /// Profil du projet ou de `~/.ssh/config` : Grove ne l'écrit pas.
    fn render_readonly_profile(&self, editor: &ProfileEditor, cx: &mut Context<Self>) -> AnyElement {
        let draft = editor.draft.clone();
        let (origin, action, hint) = match &editor.source {
            ProfileSource::Project(path) => (
                format!("Défini dans {}", path.display()),
                "Copier dans mes profils",
                "Versionné avec le projet et partagé avec l'équipe : Grove ne modifie pas ce fichier.",
            ),
            _ => (
                "Entrée Host de ~/.ssh/config".to_owned(),
                "Personnaliser",
                "Proposé tel quel, sans recopie. « Personnaliser » en crée un profil Grove qui le remplace (utilisateur, commande distante…).",
            ),
        };
        let from_ssh = editor.source == ProfileSource::SshConfig;
        let fields = [
            ("Type", match draft.kind() {
                ProfileKind::Shell => "Shell".to_owned(),
                ProfileKind::Ssh => "SSH".to_owned(),
                ProfileKind::Command => "Commande".to_owned(),
            }),
            ("Lance", detail(&draft)),
        ];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(profile_icon(&draft), theme::accent()))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(draft.name.clone()))
                    .child(div().text_xs().text_color(theme::text_faint()).child(source_label(&editor.source))),
            )
            .child(div().text_sm().text_color(theme::text_muted()).child(origin))
            .children(fields.into_iter().map(|(label, value)| editor_row(label, div().text_sm().child(value))))
            .child(div().text_xs().text_color(theme::text_faint()).child(hint))
            .child(div().pt_2().child(button("customize-profile", action, ButtonStyle::Secondary).on_click(cx.listener(move |this, _, window, cx| {
                let mut copy = draft.clone();
                // Un profil Grove du même nom masque l'hôte de ~/.ssh/config ; une copie
                // d'un profil de projet, elle, serait masquée par lui : autre nom.
                if !from_ssh {
                    copy.name = this.free_name(&copy.name);
                }
                this.new_profile(copy, window, cx);
            }))))
            .into_any_element()
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.settings.as_ref().expect("page ouverte");
        let query = settings.search.text.as_str();
        let shortcuts: Vec<_> = shortcuts::list(&crate::grove_view::key_bindings()).into_iter().filter(|s| shortcuts::matches(s, query)).collect();
        let mut groups: Vec<(shortcuts::Group, Vec<shortcuts::Shortcut>)> = Vec::new();
        for shortcut in shortcuts {
            match groups.last_mut() {
                Some((group, list)) if *group == shortcut.group => list.push(shortcut),
                _ => groups.push((shortcut.group, vec![shortcut])),
            }
        }
        let focused = settings.search.focus.clone();
        div()
            .child(section_title("Raccourcis clavier"))
            .child(
                div()
                    .id("shortcut-search")
                    .track_focus(&settings.search.focus)
                    .relative()
                    .mb_4()
                    .h(px(32.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border_strong())
                    .bg(theme::background())
                    .text_sm()
                    .cursor_text()
                    .on_click(cx.listener(move |_, _, window, _| window.focus(&focused)))
                    .child(icon(Icon::Search, theme::text_faint()))
                    .child(settings.search.text.render("Chercher une action ou une touche…"))
                    .child(text_field::input_target(&settings.search.focus, cx.entity())),
            )
            .when(groups.is_empty(), |d| d.child(div().text_sm().text_color(theme::text_muted()).child("Aucun raccourci ne correspond.")))
            .children(groups.into_iter().map(|(group, list)| {
                div()
                    .mb_4()
                    .child(div().pb_1().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(theme::text_faint()).child(group.label()))
                    .children(list.into_iter().map(|shortcut| {
                        div()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme::border())
                            .text_sm()
                            .child(div().flex_1().child(shortcut.description))
                            .child(div().flex().gap_1().children(shortcut.keys.split(", ").map(|keys| kbd(keys.to_owned()))))
                    }))
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_faint())
                    .child("Aussi : Ctrl+clic ouvre un lien du terminal, clic milieu colle la sélection, Shift garde la souris pour Grove quand un programme la capte. Pas encore personnalisables."),
            )
            .into_any_element()
    }
}

fn section_title(title: &'static str) -> impl IntoElement {
    div().pb_4().text_lg().font_weight(gpui::FontWeight::SEMIBOLD).child(title)
}

/// Réglage : libellé et explication à gauche, contrôle à droite.
fn setting_row(label: &'static str, description: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_6()
        .py_3()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).child(label))
                .child(div().text_xs().text_color(theme::text_muted()).child(description)),
        )
        .child(div().flex_none().child(control))
}

fn editor_row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .py_1()
        .child(div().w(px(130.0)).flex_none().text_sm().text_color(theme::text_muted()).child(label))
        .child(control)
}

/// Interrupteur.
fn toggle(id: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(34.0))
        .h(px(20.0))
        .flex_none()
        .p(px(2.0))
        .flex()
        .rounded_full()
        .cursor_pointer()
        .when(on, |d| d.bg(theme::accent()).justify_end())
        .when(!on, |d| d.bg(theme::border_strong()).justify_start())
        .child(div().size(px(16.0)).rounded_full().bg(if on { theme::on_accent() } else { theme::text_muted() }))
}

/// Choix exclusif : boutons accolés, celui choisi en surbrillance.
fn segmented<F>(options: impl Iterator<Item = (&'static str, bool, F)>) -> impl IntoElement
where
    F: Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
{
    div()
        .flex()
        .flex_none()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded_md()
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::background())
        .children(options.map(|(label, selected, on_click)| {
            div()
                .id(label)
                .h(px(24.0))
                .px_2p5()
                .flex()
                .items_center()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme::selected()).text_color(theme::text()))
                .when(!selected, |d| d.text_color(theme::text_muted()).hover(|s| s.text_color(theme::text())))
                .on_click(on_click)
                .child(label)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_fields_round_trip() {
        let mut profile = Profile::ssh("web");
        ProfileField::Env.set(&mut profile, " A=1, B = deux ,").unwrap();
        ProfileField::Options.set(&mut profile, "ServerAliveInterval=30, ,ForwardAgent=yes").unwrap();
        ProfileField::Port.set(&mut profile, "2222").unwrap();
        ProfileField::User.set(&mut profile, "  ").unwrap();
        assert_eq!(ProfileField::Env.get(&profile), "A=1, B=deux");
        assert_eq!(profile.options, ["ServerAliveInterval=30", "ForwardAgent=yes"]);
        assert_eq!(profile.port, Some(2222));
        assert_eq!(profile.user, None);
        assert!(ProfileField::Port.set(&mut profile, "ssh").is_err());
        assert!(ProfileField::Env.set(&mut profile, "SANS_EGAL").is_err());
    }
}
