//! Vue diff simple (specs §9.5) : fichiers modifiés de l'espace actif par
//! rapport à `HEAD`, et diff du fichier choisi. Affichée à la place des onglets.

use std::path::PathBuf;

use gpui::{
    Context, FocusHandle, Hsla, KeyDownEvent, ListHorizontalSizingBehavior, ScrollStrategy, SharedString, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use grove_git::{ChangedFile, DiffLine, DiffLineKind, FileChange, FileDiff};

use crate::grove_view::{GroveView, WorkspaceKey};
use crate::theme;
use crate::ui::{Icon, hint, icon, icon_button, tooltip};

const LINE_HEIGHT: f32 = 20.0;

pub(crate) struct DiffPanel {
    pub workspace: WorkspaceKey,
    workdir: PathBuf,
    /// `None` pendant la première lecture.
    files: Option<Vec<ChangedFile>>,
    /// Chemin du fichier affiché : il reste choisi d'une relecture à l'autre.
    selected: Option<String>,
    diff: Option<FileDiff>,
    error: Option<String>,
    pub focus: FocusHandle,
    scroll: UniformListScrollHandle,
    files_task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
}

impl DiffPanel {
    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.files.as_ref()?.iter().position(|f| f.path == selected)
    }
}

impl GroveView {
    pub(crate) fn toggle_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.take().is_some() {
            self.focus_active(window, cx);
            return cx.notify();
        }
        let Some(key) = self.state.active_workspace else { return };
        if !self.git.contains_key(&key.1) {
            return;
        }
        let Some(workdir) = self.state.workspace(key).map(|w| w.path.clone()) else { return };
        self.leave_settings(cx);
        self.github.panel = None;
        self.claude.history = None;
        self.usage_panel = None;
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.diff = Some(DiffPanel {
            workspace: key,
            workdir,
            files: None,
            selected: None,
            diff: None,
            error: None,
            focus,
            scroll: UniformListScrollHandle::new(),
            files_task: None,
            diff_task: None,
        });
        self.reload_diff_files(cx);
        cx.notify();
    }

    /// Relit la liste des fichiers modifiés, puis le diff du fichier choisi.
    pub(crate) fn reload_diff_files(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.diff else { return };
        let workdir = panel.workdir.clone();
        let work = cx.background_executor().spawn(async move { grove_git::changed_files(&workdir) });
        panel.files_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(panel) = &mut this.diff else { return };
                match result {
                    Ok(files) => {
                        let keep = panel.selected.as_deref().is_some_and(|s| files.iter().any(|f| f.path == s));
                        if !keep {
                            panel.selected = files.first().map(|f| f.path.clone());
                        }
                        panel.files = Some(files);
                        panel.error = None;
                        // Le contenu peut avoir changé sans changer les compteurs.
                        this.load_selected_diff(cx);
                    }
                    Err(err) => panel.error = Some(format!("{err:#}")),
                }
                cx.notify();
            });
        }));
    }

    fn load_selected_diff(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.diff else { return };
        let Some(file) = panel.selected_index().and_then(|i| panel.files.as_ref()?.get(i).cloned()) else {
            panel.diff = None;
            return;
        };
        let workdir = panel.workdir.clone();
        let work = cx.background_executor().spawn(async move { grove_git::file_diff(&workdir, &file) });
        panel.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(panel) = &mut this.diff else { return };
                match result {
                    Ok(diff) => panel.diff = Some(diff),
                    Err(err) => {
                        panel.diff = None;
                        panel.error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            });
        }));
    }

    fn select_diff_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.diff else { return };
        if panel.selected.as_deref() == Some(path.as_str()) {
            return;
        }
        panel.selected = Some(path);
        panel.diff = None;
        panel.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.load_selected_diff(cx);
        cx.notify();
    }

    fn diff_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.diff else { return };
        let count = panel.files.as_ref().map_or(0, Vec::len);
        let index = panel.selected_index();
        let next = match event.keystroke.key.as_str() {
            "escape" => return self.toggle_diff(window, cx),
            "up" | "k" => index.map(|i| i.saturating_sub(1)),
            "down" | "j" => index.map(|i| (i + 1).min(count.saturating_sub(1))),
            _ => return,
        };
        if let Some(path) = next.and_then(|i| panel.files.as_ref()?.get(i)).map(|f| f.path.clone()) {
            self.select_diff_file(path, cx);
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_diff(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let panel = self.diff.as_ref()?;
        let files = panel.files.as_deref().unwrap_or_default();
        let (added, removed) = files.iter().fold((0, 0), |(a, r), f| (a + f.added.unwrap_or(0), r + f.removed.unwrap_or(0)));
        let branch = self.git.get(&panel.workspace.1).and_then(|g| g.head.as_ref()).map(|h| h.label().to_owned());

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .child(icon(Icon::GitBranch, theme::text_muted()))
            .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child("Modifications"))
            .children(branch.map(|b| div().text_color(theme::text_muted()).child(format!("sur {b}, par rapport à HEAD"))))
            .child(div().flex_1())
            .when(!files.is_empty(), |d| {
                d.child(div().text_xs().text_color(theme::text_faint()).child(format!(
                    "{} fichier{}",
                    files.len(),
                    if files.len() > 1 { "s" } else { "" }
                )))
                .child(div().text_xs().text_color(theme::accent()).child(format!("+{added}")))
                .child(div().text_xs().text_color(theme::danger()).child(format!("−{removed}")))
            })
            .child(
                icon_button("diff-refresh", Icon::Refresh)
                    .tooltip(tooltip("Relire"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload_diff_files(cx))),
            )
            .child(hint("Échap", "fermer").text_xs().text_color(theme::text_faint()));

        let selected = panel.selected.as_deref();
        let list = div()
            .id("diff-files")
            .w(px(300.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme::border())
            .p_1()
            .flex()
            .flex_col()
            .children(files.iter().enumerate().map(|(i, file)| self.diff_file_row(i, file, selected == Some(file.path.as_str()), cx)));

        let body: gpui::AnyElement = match (&panel.files, &panel.error) {
            (_, Some(error)) => message(error.clone(), theme::danger()),
            (None, _) => message("Lecture des modifications…".into(), theme::text_muted()),
            (Some(files), _) if files.is_empty() => message("Aucune modification : le dossier est identique à HEAD.".into(), theme::text_muted()),
            (Some(_), _) => div().flex_1().min_w_0().h_full().flex().child(list).child(self.render_file_diff(panel, cx)).into_any_element(),
        };

        Some(
            div()
                .id("diff-view")
                .track_focus(&panel.focus)
                .on_key_down(cx.listener(Self::diff_key_down))
                .size_full()
                .flex()
                .flex_col()
                .text_sm()
                .child(header)
                .child(div().flex_1().min_h_0().flex().child(body))
                .into_any_element(),
        )
    }

    fn diff_file_row(&self, index: usize, file: &ChangedFile, selected: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (letter, color) = change_look(file.change);
        let (dir, name) = match file.path.rsplit_once('/') {
            Some((dir, name)) => (Some(format!("{dir}/")), name.to_owned()),
            None => (None, file.path.clone()),
        };
        let path = file.path.clone();
        let hint = match (&file.old_path, file.staged) {
            (Some(old), _) => format!("{old} → {}", file.path),
            (None, true) => format!("{} · modifications indexées", file.path),
            (None, false) => file.path.clone(),
        };
        div()
            .id(SharedString::from(format!("diff-file-{index}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(26.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::selected()))
            .when(!selected, |d| d.hover(|s| s.bg(theme::hover())))
            .tooltip(tooltip(hint))
            .on_click(cx.listener(move |this, _, _, cx| this.select_diff_file(path.clone(), cx)))
            .child(div().w(px(12.0)).flex_none().text_xs().font_weight(gpui::FontWeight::BOLD).text_color(color).child(letter))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(div().flex_none().text_color(theme::text()).child(name))
                    .children(dir.map(|d| div().pl_1p5().min_w_0().text_ellipsis().overflow_hidden().text_xs().text_color(theme::text_faint()).child(d))),
            )
            .children(file.added.filter(|n| *n > 0).map(|n| div().flex_none().text_xs().text_color(theme::accent()).child(format!("+{n}"))))
            .children(file.removed.filter(|n| *n > 0).map(|n| div().flex_none().text_xs().text_color(theme::danger()).child(format!("−{n}"))))
            .into_any_element()
    }

    fn render_file_diff(&self, panel: &DiffPanel, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(diff) = &panel.diff else {
            return message("Lecture du diff…".into(), theme::text_muted());
        };
        if diff.binary {
            return message("Fichier binaire.".into(), theme::text_muted());
        }
        if diff.lines.is_empty() {
            return message("Pas de différence de contenu (droits ou type de fichier).".into(), theme::text_muted());
        }
        let count = diff.lines.len();
        // La ligne la plus longue donne la largeur : les autres défilent avec elle.
        let widest = diff.lines.iter().enumerate().max_by_key(|(_, l)| l.text.chars().count()).map(|(i, _)| i);
        let family = crate::fonts::resolve_family(&self.config.font.family);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .font_family(family)
            .text_size(px(12.5))
            .child(
                uniform_list(
                    "diff-lines",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                        let Some(diff) = this.diff.as_ref().and_then(|p| p.diff.as_ref()) else { return Vec::new() };
                        range.filter_map(|i| diff.lines.get(i)).map(diff_line).collect()
                    }),
                )
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(widest)
                .track_scroll(panel.scroll.clone())
                .size_full(),
            )
            .into_any_element()
    }
}

/// Lettre et couleur d'un type de modification, comme dans `git status`.
fn change_look(change: FileChange) -> (&'static str, Hsla) {
    match change {
        FileChange::Modified => ("M", theme::waiting()),
        FileChange::Added => ("A", theme::accent()),
        FileChange::Untracked => ("U", theme::accent()),
        FileChange::Deleted => ("D", theme::danger()),
        FileChange::Renamed => ("R", theme::link()),
        FileChange::Conflicted => ("!", theme::danger()),
    }
}

fn diff_line(line: &DiffLine) -> gpui::AnyElement {
    let number = |n: Option<u32>| {
        div()
            .w(px(44.0))
            .flex_none()
            .pr_2()
            .flex()
            .justify_end()
            .text_color(theme::text_faint())
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let (sign, bg, color) = match line.kind {
        DiffLineKind::Added => ("+", Some(theme::accent().opacity(0.13)), theme::text()),
        DiffLineKind::Removed => ("−", Some(theme::danger().opacity(0.13)), theme::text()),
        DiffLineKind::Hunk => ("", Some(theme::panel()), theme::text_muted()),
        DiffLineKind::Note => ("", None, theme::text_faint()),
        DiffLineKind::Context => ("", None, theme::text_muted()),
    };
    div()
        .h(px(LINE_HEIGHT))
        .min_w_full()
        .flex()
        .items_center()
        .whitespace_nowrap()
        .when_some(bg, |d, bg| d.bg(bg))
        .child(number(line.old))
        .child(number(line.new))
        .child(div().w(px(16.0)).flex_none().text_color(color).child(sign))
        .child(div().flex_none().pr_4().text_color(color).child(if line.text.is_empty() { " ".to_owned() } else { line.text.clone() }))
        .into_any_element()
}

fn message(text: String, color: Hsla) -> gpui::AnyElement {
    div().flex_1().p_6().text_color(color).child(text).into_any_element()
}
