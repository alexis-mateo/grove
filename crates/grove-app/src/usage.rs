//! Limites d'usage du compte Claude (fenêtre de 5 h, semaine), en bas de la sidebar.
//!
//! Les données arrivent avec la statusline de n'importe quelle session : ce
//! sont des limites de compte, la dernière valeur reçue fait foi. Claude Code ne
//! les transmet qu'après la première réponse d'une session : les dernières
//! valeurs connues sont donc conservées dans `usage.json` pour être affichées
//! dès le démarrage. Aucun appel à une API d'Anthropic (specs §8.5).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    Bounds, Context, Hsla, PathBuilder, Pixels, SharedString, canvas, div, point, prelude::*, px,
};
use grove_claude::{RateLimits, RateWindow};
use serde::{Deserialize, Serialize};

use crate::grove_view::GroveView;
use crate::marquee::marquee;
use crate::theme;

const FIVE_HOURS: i64 = 5 * 3600;
const SEVEN_DAYS: i64 = 7 * 86_400;

/// Seuils d'alerte de la fenêtre de 5 h, en pourcentage.
pub const ALERT_THRESHOLDS: [f64; 2] = [90.0, 100.0];

/// Au-delà, on réécrit `usage.json` même sans changement, pour que l'âge affiché reste juste.
const RESAVE_AFTER: Duration = Duration::from_secs(300);

/// Limites connues, et seuil d'alerte déjà notifié pour la fenêtre de 5 h en cours.
#[derive(Default)]
pub(crate) struct Usage {
    pub limits: Option<RateLimits>,
    pub received_at: Option<SystemTime>,
    pub expanded: bool,
    alerted: Option<(Option<i64>, f64)>,
    /// Fichier de persistance (`None` en test).
    path: Option<PathBuf>,
    saved_at: Option<SystemTime>,
    /// Dernières limites reçues par la statusline (source officielle).
    statusline_at: Option<std::time::Instant>,
}

/// Contenu de `usage.json`.
#[derive(Serialize, Deserialize)]
struct Stored {
    limits: RateLimits,
    received_at: i64,
    /// Évite de renotifier un seuil déjà signalé avant le redémarrage.
    alerted: Option<(Option<i64>, f64)>,
}

impl Usage {
    /// Recharge les dernières limites connues. Un fichier absent ou illisible est ignoré.
    pub fn load(path: PathBuf) -> Self {
        let stored = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok());
        let mut usage = Self { path: Some(path), ..Default::default() };
        if let Some(stored) = stored {
            let received_at = UNIX_EPOCH + Duration::from_secs(stored.received_at.max(0) as u64);
            usage.limits = Some(stored.limits);
            usage.received_at = Some(received_at);
            usage.saved_at = Some(received_at);
            usage.alerted = stored.alerted;
        }
        usage
    }

    fn save(&mut self) {
        let (Some(path), Some(limits)) = (&self.path, &self.limits) else { return };
        let stored = Stored { limits: limits.clone(), received_at: now(), alerted: self.alerted };
        let tmp = path.with_extension("json.tmp");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let written = serde_json::to_vec_pretty(&stored)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(std::fs::write(&tmp, bytes)?))
            .and_then(|()| Ok(std::fs::rename(&tmp, path)?));
        match written {
            Ok(()) => self.saved_at = Some(SystemTime::now()),
            Err(err) => log::warn!("sauvegarde de {} : {err:#}", path.display()),
        }
    }

    /// Limites reçues par la statusline. Voir [`Self::update`].
    pub fn update_from_statusline(&mut self, limits: RateLimits) -> Option<f64> {
        self.statusline_at = Some(std::time::Instant::now());
        self.update(limits)
    }

    /// La statusline a fourni des limites depuis moins de `within` : l'API d'usage est inutile.
    pub fn statusline_fresh(&self, within: Duration) -> bool {
        self.statusline_at.is_some_and(|t| t.elapsed() < within)
    }

    /// Enregistre de nouvelles limites. Renvoie le seuil franchi s'il faut alerter.
    pub fn update(&mut self, limits: RateLimits) -> Option<f64> {
        let crossed = limits.five_hour.and_then(|w| {
            let threshold = ALERT_THRESHOLDS.iter().rev().find(|t| w.used_percent >= **t).copied()?;
            // Une alerte par seuil et par fenêtre (identifiée par sa date de remise à zéro).
            let already = self.alerted.is_some_and(|(reset, t)| reset == w.resets_at && t >= threshold);
            if already {
                return None;
            }
            self.alerted = Some((w.resets_at, threshold));
            Some(threshold)
        });
        let changed = self.limits.as_ref() != Some(&limits) || crossed.is_some();
        let stale = self.saved_at.and_then(|t| t.elapsed().ok()).is_none_or(|age| age >= RESAVE_AFTER);
        self.limits = Some(limits);
        self.received_at = Some(SystemTime::now());
        if changed || stale {
            self.save();
        }
        crossed
    }
}

/// La fenêtre est déjà remise à zéro : la valeur connue ne veut plus rien dire.
fn expired(window: &RateWindow) -> bool {
    window.resets_at.is_some_and(|at| at <= now())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn level_color(percent: f64) -> Hsla {
    match percent {
        p if p >= 90.0 => theme::danger(),
        p if p >= 70.0 => theme::waiting(),
        _ => theme::accent(),
    }
}

/// Part écoulée de la fenêtre (0–1), d'après sa date de remise à zéro.
fn elapsed_fraction(window: &RateWindow, length: i64) -> Option<f32> {
    let remaining = window.resets_at? - now();
    Some((1.0 - remaining as f32 / length as f32).clamp(0.0, 1.0))
}

/// « 2 h 14 », « 14 min », « 3 j 4 h ».
pub fn countdown(resets_at: i64) -> String {
    let secs = (resets_at - now()).max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{} min", minutes.max(1)),
        (0, h) => format!("{h} h {minutes:02}"),
        (d, h) => format!("{d} j {h} h"),
    }
}

/// « aujourd'hui à 14:00 », « demain à 09:30 », « lun. 27 oct. à 14:00 ».
pub fn reset_date(resets_at: i64) -> String {
    use jiff::{Timestamp, tz::TimeZone};
    let tz = TimeZone::system();
    let (Ok(at), Ok(today)) = (Timestamp::from_second(resets_at), Timestamp::from_second(now())) else {
        return String::new();
    };
    let (at, today) = (at.to_zoned(tz.clone()), today.to_zoned(tz));
    let time = format!("{:02}:{:02}", at.hour(), at.minute());
    let days = (at.date() - today.date()).get_days();
    match days {
        0 => format!("aujourd'hui à {time}"),
        1 => format!("demain à {time}"),
        _ => {
            const WEEKDAYS: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
            const MONTHS: [&str; 12] =
                ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
            let weekday = WEEKDAYS[at.weekday().to_monday_zero_offset() as usize];
            let month = MONTHS[(at.month() - 1) as usize];
            format!("{weekday} {} {month} à {time}", at.day())
        }
    }
}

/// Anneau de progression, rempli dans le sens horaire depuis midi.
fn ring(percent: f64, size: Pixels) -> impl IntoElement {
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    let color = level_color(percent);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let stroke = px(2.5);
            let center = bounds.center();
            let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2.0 - f32::from(stroke) / 2.0;
            let arc = |from: f32, to: f32| {
                let steps = ((to - from) * 64.0).ceil().max(1.0) as usize;
                let mut path = PathBuilder::stroke(stroke);
                for i in 0..=steps {
                    let angle = (from + (to - from) * i as f32 / steps as f32) * std::f32::consts::TAU
                        - std::f32::consts::FRAC_PI_2;
                    let p = point(center.x + px(radius * angle.cos()), center.y + px(radius * angle.sin()));
                    if i == 0 { path.move_to(p) } else { path.line_to(p) }
                }
                path.build().ok()
            };
            if let Some(track) = arc(0.0, 1.0) {
                window.paint_path(track, theme::selected());
            }
            if fraction > 0.0
                && let Some(progress) = arc(0.0, fraction)
            {
                window.paint_path(progress, color);
            }
        },
    )
    .size(size)
}

/// Barre horizontale, avec un repère du temps écoulé dans la fenêtre.
fn bar(percent: f64, elapsed: Option<f32>) -> impl IntoElement {
    let fill = (percent / 100.0).clamp(0.0, 1.0) as f32;
    div()
        .relative()
        .h(px(4.0))
        .w_full()
        .rounded_full()
        .bg(theme::selected())
        .child(div().h_full().w(gpui::relative(fill)).rounded_full().bg(level_color(percent)))
        .children(elapsed.map(|e| {
            div()
                .absolute()
                .top(px(-3.0))
                .left(gpui::relative(e))
                .w(px(1.0))
                .h(px(10.0))
                .bg(theme::text_muted())
        }))
}

impl GroveView {
    pub(crate) fn render_usage(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let limits = self.claude.usage.limits.as_ref()?;
        let five = limits.five_hour?;
        let five_expired = expired(&five);

        let summary = div()
            .id("usage-summary")
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_md()
            .hover(|s| s.bg(theme::hover()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.claude.usage.expanded = !this.claude.usage.expanded;
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .size(px(28.0))
                    .flex_none()
                    .child(ring(if five_expired { 0.0 } else { five.used_percent }, px(28.0)))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.0))
                            .text_color(theme::text_muted())
                            .child(if five_expired { "0".to_owned() } else { format!("{:.0}", five.used_percent) }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .text_xs()
                    .child(line("usage-five-title", if five_expired {
                        "Limite 5 h · réinitialisée".to_owned()
                    } else {
                        format!("Limite 5 h · {:.0} %", five.used_percent)
                    }).text_color(theme::text()))
                    .child(line("usage-five-reset", match five.resets_at {
                        _ if five_expired => "nouvelle fenêtre".to_owned(),
                        Some(at) => format!("reset dans {}", countdown(at)),
                        None => "reset inconnu".to_owned(),
                    }).text_color(theme::text_faint())),
            )
            .children(limits.seven_day.filter(|w| !expired(w)).map(|week| {
                div()
                    .flex_none()
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(if week.used_percent >= 70.0 { level_color(week.used_percent) } else { theme::text_faint() })
                    .child(format!("sem. {:.0} %", week.used_percent))
            }));

        let detail = self.claude.usage.expanded.then(|| {
            let window_row = |label: &'static str, window: RateWindow, length: i64| {
                let is_expired = expired(&window);
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(line(SharedString::from(format!("usage-label-{label}")), label).flex_1().text_color(theme::text_muted()))
                            .child(div().flex_none().whitespace_nowrap().text_color(theme::text()).child(if is_expired {
                                "réinitialisée".to_owned()
                            } else {
                                format!("{:.0} %", window.used_percent)
                            })),
                    )
                    .child(if is_expired { bar(0.0, None) } else { bar(window.used_percent, elapsed_fraction(&window, length)) })
                    .children(window.resets_at.filter(|_| !is_expired).map(|at| {
                        line(SharedString::from(format!("usage-reset-{label}")), format!("reset {} (dans {})", reset_date(at), countdown(at)))
                            .text_color(theme::text_faint())
                    }))
            };
            let updated: SharedString = match self.claude.usage.received_at.and_then(|t| t.elapsed().ok()) {
                Some(d) if d.as_secs() >= 86_400 => format!("mis à jour il y a {} j", d.as_secs() / 86_400).into(),
                Some(d) if d.as_secs() >= 3600 => format!("mis à jour il y a {} h", d.as_secs() / 3600).into(),
                Some(d) if d.as_secs() >= 120 => format!("mis à jour il y a {} min", d.as_secs() / 60).into(),
                _ => "à jour".into(),
            };
            div()
                .flex()
                .flex_col()
                .gap_3()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .child(window_row("Fenêtre de 5 h", five, FIVE_HOURS))
                .children(limits.seven_day.map(|w| window_row("Semaine, tous modèles", w, SEVEN_DAYS)))
                .children(limits.seven_day_opus.map(|w| window_row("Semaine, Opus", w, SEVEN_DAYS)))
                .children(limits.seven_day_sonnet.map(|w| window_row("Semaine, Sonnet", w, SEVEN_DAYS)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .text_color(theme::text_faint())
                        .child(line("usage-legend", "│ repère : temps écoulé"))
                        .child(line("usage-updated", updated)),
                )
        });

        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .border_t_1()
                .border_color(theme::border())
                .children(detail)
                .child(summary)
                .into_any_element(),
        )
    }
}

/// Une ligne de texte, tronquée avec « … » (et lisible en entier au survol)
/// plutôt que renvoyée à la ligne : la hauteur de la carte ne dépend ainsi pas
/// de la largeur de la sidebar.
fn line(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> gpui::Div {
    div().flex().w_full().min_w_0().child(marquee(id, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(percent: f64, resets_at: i64) -> RateLimits {
        RateLimits { five_hour: Some(RateWindow { used_percent: percent, resets_at: Some(resets_at) }), ..Default::default() }
    }

    #[test]
    fn alerts_once_per_threshold_and_window() {
        let mut usage = Usage::default();
        assert_eq!(usage.update(limits(50.0, 1)), None);
        assert_eq!(usage.update(limits(91.0, 1)), Some(90.0));
        assert_eq!(usage.update(limits(95.0, 1)), None);
        assert_eq!(usage.update(limits(100.0, 1)), Some(100.0));
        assert_eq!(usage.update(limits(100.0, 1)), None);
        // Nouvelle fenêtre : on peut réalerter.
        assert_eq!(usage.update(limits(92.0, 2)), Some(90.0));
    }

    #[test]
    fn persists_and_reloads_without_realerting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let mut usage = Usage::load(path.clone());
        assert!(usage.limits.is_none());
        assert_eq!(usage.update(limits(93.0, now() + 600)), Some(90.0));

        let mut reloaded = Usage::load(path);
        assert_eq!(reloaded.limits, usage.limits);
        assert!(reloaded.received_at.is_some());
        // Même fenêtre, même seuil : pas de nouvelle alerte après redémarrage.
        assert_eq!(reloaded.update(limits(94.0, now() + 600)), None);
    }

    #[test]
    fn past_reset_means_expired() {
        assert!(expired(&RateWindow { used_percent: 80.0, resets_at: Some(now() - 1) }));
        assert!(!expired(&RateWindow { used_percent: 80.0, resets_at: Some(now() + 60) }));
        assert!(!expired(&RateWindow { used_percent: 80.0, resets_at: None }));
    }

    #[test]
    fn countdown_formats() {
        assert_eq!(countdown(now() + 2 * 3600 + 14 * 60 + 30), "2 h 14");
        assert_eq!(countdown(now() + 5 * 60 + 30), "5 min");
        assert_eq!(countdown(now() + 3 * 86_400 + 4 * 3600 + 60), "3 j 4 h");
        assert_eq!(countdown(now() - 10), "1 min");
    }
}
