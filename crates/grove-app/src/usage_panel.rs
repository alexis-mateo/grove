//! Usage de Claude Code par jour et par projet (specs §8.5, « plus tard »),
//! lu dans les transcripts. Coût estimé au tarif de l'API : avec un abonnement,
//! ce n'est pas ce qui est facturé, mais un ordre de grandeur comparable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gpui::{Context, FocusHandle, Hsla, KeyDownEvent, SharedString, Task, Window, div, prelude::*, px};
use grove_claude::usage_history::{Record, Scanner, Tokens};
use grove_core::ProjectId;
use jiff::civil::Date;

use crate::grove_view::GroveView;
use crate::theme;
use crate::ui::{Icon, hint, icon, icon_button, tooltip};

/// Périodes proposées, en jours.
const PERIODS: [u32; 3] = [7, 30, 90];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Totals {
    pub cost: f64,
    pub tokens: Tokens,
    pub requests: u32,
    /// Réponses d'un modèle sans tarif connu (comptées en tokens seulement).
    pub unpriced: u32,
}

impl Totals {
    fn add(&mut self, other: &Totals) {
        self.cost += other.cost;
        self.tokens.add(&other.tokens);
        self.requests += other.requests;
        self.unpriced += other.unpriced;
    }
}

/// Une case du tableau : un jour, un projet (ou un dossier hors projet), un modèle.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cell {
    pub day: Date,
    pub project: Option<ProjectId>,
    /// Nom du projet, ou dossier (`~/x`) hors des projets de Grove.
    pub label: String,
    pub model: String,
    pub totals: Totals,
}

pub(crate) struct UsagePanel {
    pub focus: FocusHandle,
    /// Tous les projets, sinon le projet actif.
    pub all: bool,
    pub days: u32,
    cells: Option<Vec<Cell>>,
    loaded_at: Option<SystemTime>,
    loading: bool,
    _task: Option<Task<()>>,
}

/// Dossiers d'un projet : sa racine, ses espaces et `<racine>.worktrees`.
struct ProjectDirs {
    id: ProjectId,
    name: String,
    dirs: Vec<PathBuf>,
}

impl GroveView {
    pub(crate) fn toggle_usage_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage_panel.take().is_some() {
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.leave_settings(cx);
        self.github.panel = None;
        self.claude.history = None;
        self.diff = None;
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.usage_panel = Some(UsagePanel {
            focus,
            all: self.state.active_workspace.is_none(),
            days: PERIODS[0],
            cells: None,
            loaded_at: None,
            loading: false,
            _task: None,
        });
        self.reload_usage_history(cx);
        cx.notify();
    }

    /// Relit les transcripts (seulement ce qui a été ajouté depuis la dernière fois).
    fn reload_usage_history(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.usage_panel else { return };
        if panel.loading {
            return;
        }
        panel.loading = true;
        let scanner = self.usage_scanner.clone();
        let projects: Vec<ProjectDirs> = self
            .state
            .projects
            .iter()
            .map(|p| {
                let mut dirs = vec![p.root.clone()];
                dirs.extend(p.workspaces.iter().map(|w| w.path.clone()));
                if let Some(name) = p.root.file_name() {
                    let mut sibling = name.to_owned();
                    sibling.push(".worktrees");
                    dirs.push(p.root.with_file_name(sibling));
                }
                ProjectDirs { id: p.id, name: p.name.clone(), dirs }
            })
            .collect();
        let work = cx.background_executor().spawn(async move {
            let records = scanner.lock().map(|mut s| s.scan()).unwrap_or_default();
            aggregate(&records, &projects, std::env::var_os("HOME").map(PathBuf::from).as_deref())
        });
        panel._task = Some(cx.spawn(async move |this, cx| {
            let cells = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = &mut this.usage_panel {
                    panel.cells = Some(cells);
                    panel.loaded_at = Some(SystemTime::now());
                    panel.loading = false;
                    cx.notify();
                }
            });
        }));
    }

    fn usage_panel_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.toggle_usage_panel(window, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_usage_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let panel = self.usage_panel.as_ref()?;
        let active_project = self.state.active_workspace.map(|(p, _)| p);
        let today = jiff::Zoned::now().date();
        let first = today.checked_sub(jiff::Span::new().days(i64::from(panel.days) - 1)).unwrap_or(today);
        let cells: Vec<&Cell> = panel
            .cells
            .iter()
            .flatten()
            .filter(|c| c.day >= first && c.day <= today && (panel.all || c.project == active_project))
            .collect();

        let mut total = Totals::default();
        let mut by_day: BTreeMap<Date, Totals> = BTreeMap::new();
        let mut by_project: BTreeMap<&str, Totals> = BTreeMap::new();
        let mut by_model: BTreeMap<&str, Totals> = BTreeMap::new();
        for cell in &cells {
            total.add(&cell.totals);
            by_day.entry(cell.day).or_default().add(&cell.totals);
            by_project.entry(cell.label.as_str()).or_default().add(&cell.totals);
            by_model.entry(cell.model.as_str()).or_default().add(&cell.totals);
        }

        let project_name = active_project.and_then(|p| self.state.project(p)).map(|p| p.name.clone());
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .child(icon(Icon::Claude, theme::text_muted()))
            .child(div().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child("Usage Claude"))
            .children(project_name.map(|name| {
                segmented(
                    "usage-scope",
                    [(name, !panel.all), ("Tous les projets".to_owned(), panel.all)],
                    cx.listener(|this, index: &usize, _, cx| {
                        if let Some(panel) = &mut this.usage_panel {
                            panel.all = *index == 1;
                            cx.notify();
                        }
                    }),
                )
            }))
            .child(segmented(
                "usage-period",
                PERIODS.map(|d| (format!("{d} j"), panel.days == d)),
                cx.listener(|this, index: &usize, _, cx| {
                    if let Some(panel) = &mut this.usage_panel {
                        panel.days = PERIODS[*index];
                        cx.notify();
                    }
                }),
            ))
            .child(div().flex_1())
            .child(div().text_xs().text_color(theme::text_faint()).child(match (panel.loading, panel.loaded_at) {
                (true, _) => "lecture des transcripts…".to_owned(),
                (false, Some(at)) => crate::claude::relative_time(at),
                _ => String::new(),
            }))
            .child(
                icon_button("usage-refresh", Icon::Refresh)
                    .tooltip(tooltip("Relire les transcripts"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload_usage_history(cx))),
            )
            .child(hint("Échap", "fermer").text_xs().text_color(theme::text_faint()));

        let mut body = div().id("usage-history-body").flex_1().overflow_y_scroll().pb_6().flex().flex_col().gap_1();
        if panel.cells.is_none() {
            body = body.child(div().px_3().py_2().text_color(theme::text_muted()).child("Lecture des transcripts de Claude Code…"));
        } else {
            body = body
                .child(summary(&total, panel.days))
                .child(section("Par jour"))
                .children(day_rows(&by_day, first, today))
                .when(panel.all, |d| d.child(section("Par projet")).children(share_rows("project", &by_project, total.cost)))
                .child(section("Par modèle"))
                .children(share_rows("model", &by_model, total.cost))
                .child(
                    div()
                        .px_3()
                        .pt_4()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child("Coût estimé au tarif public de l'API, à partir des tokens de chaque réponse dans les transcripts (sous-agents compris). Avec un abonnement claude.ai, ce n'est pas ce qui est facturé."),
                );
        }

        Some(
            div()
                .id("usage-history-view")
                .track_focus(&panel.focus)
                .on_key_down(cx.listener(Self::usage_panel_key_down))
                .size_full()
                .flex()
                .justify_center()
                .text_sm()
                .child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .h_full()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .px_6()
                        .pt_6()
                        .child(header)
                        .child(div().h(px(1.0)).mx_3().bg(theme::border()))
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

/// Range chaque réponse dans son projet de Grove (le plus précis), ou sous son dossier.
fn aggregate(records: &[Record], projects: &[ProjectDirs], home: Option<&Path>) -> Vec<Cell> {
    let mut cells: BTreeMap<(Date, String, String), Cell> = BTreeMap::new();
    for record in records {
        let cwd = Path::new(&record.cwd);
        let project = projects
            .iter()
            .flat_map(|p| p.dirs.iter().map(move |d| (p, d)))
            .filter(|(_, dir)| cwd.starts_with(dir))
            .max_by_key(|(_, dir)| dir.as_os_str().len())
            .map(|(p, _)| p);
        let label = match project {
            Some(p) => p.name.clone(),
            None => match home.and_then(|h| cwd.strip_prefix(h).ok()) {
                Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
                _ if record.cwd.is_empty() => "(dossier inconnu)".to_owned(),
                _ => record.cwd.clone(),
            },
        };
        let cell = cells.entry((record.day, label.clone(), record.model.clone())).or_insert_with(|| Cell {
            day: record.day,
            project: project.map(|p| p.id),
            label,
            model: record.model.clone(),
            totals: Totals::default(),
        });
        cell.totals.add(&Totals {
            cost: record.cost_usd.unwrap_or(0.0),
            tokens: record.tokens,
            requests: 1,
            unpriced: u32::from(record.cost_usd.is_none()),
        });
    }
    cells.into_values().collect()
}

/// Boutons exclusifs côte à côte ; `on_select` reçoit l'index choisi.
fn segmented<const N: usize>(
    id: &'static str,
    options: [(String, bool); N],
    on_select: impl Fn(&usize, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let on_select = std::rc::Rc::new(on_select);
    div().flex().flex_none().p_0p5().gap_0p5().rounded_md().border_1().border_color(theme::border_strong()).children(
        options.into_iter().enumerate().map(move |(index, (label, active))| {
            let on_select = on_select.clone();
            div()
                .id(SharedString::from(format!("{id}-{index}")))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .when(active, |d| d.bg(theme::selected()).text_color(theme::text()))
                .when(!active, |d| d.text_color(theme::text_muted()).hover(|s| s.bg(theme::hover())))
                .on_click(move |_, window, cx| on_select(&index, window, cx))
                .child(label)
        }),
    )
}

fn summary(total: &Totals, days: u32) -> impl IntoElement {
    let stat = |value: String, label: &'static str| {
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(div().text_lg().text_color(theme::text()).font_weight(gpui::FontWeight::MEDIUM).child(value))
            .child(div().text_xs().text_color(theme::text_faint()).child(label))
    };
    div()
        .flex()
        .flex_wrap()
        .gap_8()
        .px_3()
        .py_2()
        .child(stat(money(total.cost), "coût estimé"))
        .child(stat(money(total.cost / f64::from(days.max(1))), "par jour en moyenne"))
        .child(stat(tokens(total.tokens.total()), "tokens (cache compris)"))
        .child(stat(total.requests.to_string(), "réponses"))
        .when(total.unpriced > 0, |d| d.child(stat(total.unpriced.to_string(), "sans tarif connu")))
}

fn section(title: &'static str) -> impl IntoElement {
    div().px_3().pt_4().pb_1().text_xs().font_weight(gpui::FontWeight::MEDIUM).text_color(theme::text_muted()).child(title)
}

/// Un jour par ligne, du plus récent au plus ancien, jours sans activité compris.
fn day_rows(by_day: &BTreeMap<Date, Totals>, first: Date, today: Date) -> Vec<gpui::AnyElement> {
    let max = by_day.values().map(|t| t.cost).fold(0.0, f64::max);
    let mut rows = Vec::new();
    let mut day = today;
    while day >= first {
        let totals = by_day.get(&day).copied().unwrap_or_default();
        rows.push(bar_row(
            SharedString::from(format!("usage-day-{day}")),
            day_label(day, today),
            if max > 0.0 { totals.cost / max } else { 0.0 },
            &totals,
            theme::accent(),
        ));
        let Ok(previous) = day.yesterday() else { break };
        day = previous;
    }
    rows
}

/// Une ligne par projet ou par modèle, du plus coûteux au moins coûteux.
fn share_rows(kind: &str, groups: &BTreeMap<&str, Totals>, total: f64) -> Vec<gpui::AnyElement> {
    let mut groups: Vec<_> = groups.iter().collect();
    groups.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(b.1.tokens.total().cmp(&a.1.tokens.total())));
    groups
        .into_iter()
        .map(|(label, totals)| {
            let share = if total > 0.0 { totals.cost / total } else { 0.0 };
            bar_row(SharedString::from(format!("usage-{kind}-{label}")), (*label).to_owned(), share, totals, theme::link())
        })
        .collect()
}

fn bar_row(id: SharedString, label: String, fraction: f64, totals: &Totals, color: Hsla) -> gpui::AnyElement {
    let empty = totals.requests == 0;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .h(px(26.0))
        .rounded_md()
        .hover(|s| s.bg(theme::hover()))
        .tooltip(tooltip(format!(
            "{} entrée · {} sortie · {} écrits en cache · {} lus en cache · {} réponses",
            tokens(totals.tokens.input),
            tokens(totals.tokens.output),
            tokens(totals.tokens.cache_write),
            tokens(totals.tokens.cache_read),
            totals.requests
        )))
        .child(
            div()
                .w(px(180.0))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if empty { theme::text_faint() } else { theme::text() })
                .child(label),
        )
        .child(
            div().flex_1().min_w(px(60.0)).h(px(8.0)).rounded_full().bg(theme::hover()).child(
                div().h_full().rounded_full().bg(color).w(gpui::relative(fraction.clamp(0.0, 1.0) as f32)),
            ),
        )
        .child(
            div()
                .w(px(80.0))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(if empty { theme::text_faint() } else { theme::text() })
                .child(if empty { "—".to_owned() } else { money(totals.cost) }),
        )
        .child(
            div()
                .w(px(90.0))
                .flex_none()
                .flex()
                .justify_end()
                .text_xs()
                .text_color(theme::text_faint())
                .child(if empty { String::new() } else { tokens(totals.tokens.total()) }),
        )
        .into_any_element()
}

fn money(dollars: f64) -> String {
    format!("{dollars:.2} $").replace('.', ",")
}

/// « 11,4 M » : tokens arrondis.
fn tokens(count: u64) -> String {
    let count = count as f64;
    let text = match count {
        c if c >= 1e9 => format!("{:.1} G", c / 1e9),
        c if c >= 1e6 => format!("{:.1} M", c / 1e6),
        c if c >= 1e3 => format!("{:.0} k", c / 1e3),
        c => format!("{c:.0}"),
    };
    text.replace('.', ",")
}

/// « aujourd'hui », « hier », puis « lun. 3 nov. ».
fn day_label(day: Date, today: Date) -> String {
    const WEEKDAYS: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
    const MONTHS: [&str; 12] = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
    if day == today {
        return "aujourd'hui".to_owned();
    }
    if today.yesterday().ok() == Some(day) {
        return "hier".to_owned();
    }
    let weekday = WEEKDAYS[day.weekday().to_monday_zero_offset() as usize];
    format!("{weekday} {} {}", day.day(), MONTHS[day.month() as usize - 1])
}

/// Lecteur des transcripts partagé entre deux ouvertures du panneau : la
/// relecture ne lit que ce qui a été ajouté.
pub(crate) fn new_scanner() -> Arc<Mutex<Scanner>> {
    Arc::new(Mutex::new(Scanner::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(day: &str, cwd: &str, model: &str, cost: Option<f64>) -> Record {
        Record {
            day: day.parse().unwrap(),
            cwd: cwd.into(),
            model: model.into(),
            tokens: Tokens { input: 10, output: 5, cache_write: 0, cache_read: 100 },
            cost_usd: cost,
        }
    }

    #[test]
    fn records_go_to_the_most_specific_project() {
        let app = ProjectId::new();
        let lib = ProjectId::new();
        let projects = [
            ProjectDirs { id: app, name: "app".into(), dirs: vec!["/w/app".into(), "/w/app.worktrees".into()] },
            ProjectDirs { id: lib, name: "lib".into(), dirs: vec!["/w/app/vendor/lib".into()] },
        ];
        let records = [
            record("2026-11-05", "/w/app", "claude-opus-5-5", Some(1.0)),
            record("2026-11-05", "/w/app.worktrees/feat-1", "claude-opus-5-5", Some(2.0)),
            record("2026-11-05", "/w/app/vendor/lib/src", "claude-opus-5-5", Some(0.5)),
            record("2026-11-04", "/home/me/notes", "claude-haiku-5-5", None),
        ];
        let cells = aggregate(&records, &projects, Some(Path::new("/home/me")));
        let got: Vec<_> = cells.iter().map(|c| (c.day.to_string(), c.label.as_str(), c.project, c.totals.cost, c.totals.requests, c.totals.unpriced)).collect();
        assert_eq!(
            got,
            [
                ("2026-11-04".into(), "~/notes", None, 0.0, 1, 1),
                ("2026-11-05".into(), "app", Some(app), 3.0, 2, 0),
                ("2026-11-05".into(), "lib", Some(lib), 0.5, 1, 0),
            ]
        );
    }

    #[test]
    fn labels_and_numbers_read_in_french() {
        let today: Date = "2026-11-06".parse().unwrap();
        assert_eq!(day_label(today, today), "aujourd'hui");
        assert_eq!(day_label("2026-11-05".parse().unwrap(), today), "hier");
        assert_eq!(day_label("2026-11-03".parse().unwrap(), today), "mar. 3 nov.");
        assert_eq!(money(12.345), "12,35 $");
        assert_eq!((tokens(11_420_119), tokens(950), tokens(2_400)), ("11,4 M".into(), "950".into(), "2 k".into()));
    }
}
