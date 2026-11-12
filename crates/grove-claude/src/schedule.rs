//! Tâches planifiées d'une session Claude Code : `CronCreate` (crons de session,
//! `/loop` à intervalle) et `ScheduleWakeup` (`/loop` sans intervalle).
//!
//! Claude Code les garde en mémoire, sans fichier. Grove les reconstitue à
//! partir des hooks `PostToolUse` (entrée et réponse de l'outil) ; elles
//! disparaissent avec le processus `claude` (`SessionEnd`, nouveau démarrage).

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use serde_json::Value;

/// Un cron récurrent expire 7 jours après sa création (dernier déclenchement compris).
pub const RECURRING_LIFETIME: i64 = 7 * 86_400;
/// Une tâche unique ne part que si la session est inactive : on la garde un
/// moment après l'heure prévue, faute d'événement qui confirme son départ.
const ONE_SHOT_GRACE: i64 = 10 * 60;

#[derive(Clone, Debug, PartialEq)]
pub enum ScheduleKind {
    Cron { expr: String, recurring: bool },
    /// Réveil d'une boucle `/loop` dynamique ; un seul à la fois par session.
    Wakeup { reason: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledTask {
    pub id: String,
    pub kind: ScheduleKind,
    pub prompt: String,
    /// Secondes depuis l'epoch Unix, comme les autres dates.
    pub created_at: i64,
    /// Premier déclenchement annoncé par Claude Code (décalage par tâche inclus).
    pub first_run: Option<i64>,
    /// Dernier prompt de la session identique à celui de la tâche.
    pub last_run: Option<i64>,
    /// Annulation demandée à Claude depuis Grove, pas encore confirmée.
    pub cancelling: bool,
}

const WAKEUP_ID: &str = "wakeup";

impl ScheduledTask {
    /// D'après l'entrée et la réponse de `CronCreate`.
    pub fn from_cron_create(input: &Value, response: &Value, now: i64) -> Option<Self> {
        let expr = input.get("cron").and_then(Value::as_str)?.trim().to_owned();
        let id = match response {
            Value::Object(_) => response.get("id").and_then(Value::as_str).map(str::to_owned),
            // Réponse textuelle : « Scheduled recurring job 1df0eb1a (…) ».
            Value::String(text) => id_from_text(text),
            _ => None,
        }?;
        let recurring = response
            .get("recurring")
            .or_else(|| input.get("recurring"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Some(Self {
            id,
            kind: ScheduleKind::Cron { expr, recurring },
            prompt: input.get("prompt").and_then(Value::as_str).unwrap_or_default().to_owned(),
            created_at: now,
            first_run: response.get("nextFireAt").and_then(Value::as_i64).map(|ms| ms / 1000),
            last_run: None,
            cancelling: false,
        })
    }

    /// D'après l'entrée de `ScheduleWakeup`. `None` si la boucle s'arrête.
    pub fn from_wakeup(input: &Value, now: i64) -> Option<Self> {
        if input.get("stop").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        // Le runtime borne le délai à [60, 3600] s.
        let delay = input.get("delaySeconds").and_then(Value::as_f64)?.clamp(60.0, 3600.0) as i64;
        Some(Self {
            id: WAKEUP_ID.into(),
            kind: ScheduleKind::Wakeup {
                reason: input.get("reason").and_then(Value::as_str).map(str::to_owned),
            },
            prompt: input.get("prompt").and_then(Value::as_str).unwrap_or_default().to_owned(),
            created_at: now,
            first_run: Some(now + delay),
            last_run: None,
            cancelling: false,
        })
    }

    pub fn is_recurring(&self) -> bool {
        matches!(self.kind, ScheduleKind::Cron { recurring: true, .. })
    }

    /// Encore planifiée à `now` ?
    pub fn is_active(&self, now: i64) -> bool {
        if self.is_recurring() {
            return now < self.created_at + RECURRING_LIFETIME;
        }
        self.last_run.is_none() && now < self.first_run.unwrap_or(self.created_at) + ONE_SHOT_GRACE
    }

    /// Prochain déclenchement et s'il est approché (cron recalculé sans le
    /// décalage propre à la tâche). `None` : l'heure est passée, la tâche attend
    /// que la session soit inactive.
    pub fn next_run(&self, now: i64) -> Option<(i64, bool)> {
        if let Some(first) = self.first_run.filter(|&t| t > now) {
            return Some((first, false));
        }
        match &self.kind {
            ScheduleKind::Cron { expr, recurring: true } => {
                let next = Cron::parse(expr)?.next_after(now, &TimeZone::system())?;
                (next < self.created_at + RECURRING_LIFETIME).then_some((next, true))
            }
            _ => None,
        }
    }

    /// Fréquence lisible : « toutes les 5 min », « en semaine à 09:03 »…
    pub fn describe(&self) -> String {
        match &self.kind {
            ScheduleKind::Cron { expr, recurring: true } => describe_cron(expr),
            ScheduleKind::Cron { recurring: false, .. } => "une fois".into(),
            ScheduleKind::Wakeup { .. } => "réveil /loop".into(),
        }
    }

    /// Prompt qui demande à Claude d'annuler la tâche : Grove n'a pas accès
    /// à la mémoire du processus `claude`.
    pub fn cancel_prompt(&self) -> String {
        match self.kind {
            ScheduleKind::Cron { .. } => {
                format!("Annule la tâche planifiée {} (CronDelete), sans rien faire d'autre.", self.id)
            }
            ScheduleKind::Wakeup { .. } => {
                "Arrête la boucle /loop en cours (ScheduleWakeup avec stop: true), sans rien faire d'autre.".into()
            }
        }
    }

    /// Ce que fera la tâche : son prompt, ou la raison d'un réveil.
    pub fn summary(&self) -> &str {
        match &self.kind {
            ScheduleKind::Wakeup { reason: Some(reason) } if !reason.is_empty() => reason,
            _ => &self.prompt,
        }
    }
}

/// Liste des tâches d'une session, tenue à jour par les hooks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    tasks: Vec<ScheduledTask>,
}

impl Schedule {
    /// Applique un hook. `true` si la liste a changé.
    pub fn apply_hook(&mut self, payload: &Value, now: i64) -> bool {
        let before = self.tasks.clone();
        let str_field = |key: &str| payload.get(key).and_then(Value::as_str);
        match str_field("hook_event_name") {
            // Nouveau processus `claude` : ses tâches repartent de zéro.
            Some("SessionStart") if matches!(str_field("source"), Some("startup" | "resume")) => self.tasks.clear(),
            Some("SessionEnd") => self.tasks.clear(),
            Some("PostToolUse") => {
                let input = payload.get("tool_input").unwrap_or(&Value::Null);
                match str_field("tool_name") {
                    Some("CronCreate") => {
                        let response = payload.get("tool_response").unwrap_or(&Value::Null);
                        if let Some(task) = ScheduledTask::from_cron_create(input, response, now) {
                            self.tasks.retain(|t| t.id != task.id);
                            self.tasks.push(task);
                        }
                    }
                    Some("CronDelete") => {
                        if let Some(id) = input.get("id").and_then(Value::as_str) {
                            self.tasks.retain(|t| t.id != id);
                        }
                    }
                    Some("ScheduleWakeup") => {
                        self.tasks.retain(|t| t.id != WAKEUP_ID);
                        self.tasks.extend(ScheduledTask::from_wakeup(input, now));
                    }
                    _ => {}
                }
            }
            // Tour fini sans que l'outil d'annulation soit passé : on peut réessayer.
            Some("Stop" | "StopFailure") => self.tasks.iter_mut().for_each(|t| t.cancelling = false),
            Some("UserPromptSubmit") => {
                if let Some(prompt) = str_field("prompt").map(str::trim) {
                    for task in &mut self.tasks {
                        if !task.prompt.is_empty() && task.prompt.trim() == prompt {
                            task.last_run = Some(now);
                        }
                    }
                }
            }
            _ => {}
        }
        self.tasks.retain(|t| t.is_active(now));
        self.tasks != before
    }

    /// Tâches encore planifiées, la plus proche d'abord.
    pub fn active(&self, now: i64) -> Vec<&ScheduledTask> {
        let mut tasks: Vec<_> = self.tasks.iter().filter(|t| t.is_active(now)).collect();
        tasks.sort_by_key(|t| t.next_run(now).map_or(i64::MIN, |(at, _)| at));
        tasks
    }

    /// Note qu'une annulation a été demandée. `false` si la tâche n'existe plus.
    pub fn mark_cancelling(&mut self, id: &str) -> bool {
        let task = self.tasks.iter_mut().find(|t| t.id == id);
        task.map(|t| t.cancelling = true).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

fn id_from_text(text: &str) -> Option<String> {
    let rest = text.split_once(" job ").or_else(|| text.split_once(" task "))?.1;
    let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    (!id.is_empty()).then_some(id)
}

/// Expression cron à 5 champs (minute heure jour mois jour-de-semaine), heure locale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    /// Jour du mois et jour de semaine tous deux restreints : l'un OU l'autre suffit.
    either_day: bool,
}

impl Cron {
    fn parse(expr: &str) -> Option<Self> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else { return None };
        let mut weekdays = field(weekday, 0, 7)?;
        // 7 = dimanche, comme 0.
        if weekdays & (1 << 7) != 0 {
            weekdays = (weekdays | 1) & !(1 << 7);
        }
        Some(Self {
            minutes: field(minute, 0, 59)?,
            hours: field(hour, 0, 23)?,
            days: field(day, 1, 31)?,
            months: field(month, 1, 12)?,
            weekdays,
            either_day: !day.starts_with('*') && !weekday.starts_with('*'),
        })
    }

    fn matches_date(&self, date: Date) -> bool {
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().to_sunday_zero_offset()) != 0;
        let month = self.months & (1 << date.month()) != 0;
        month && if self.either_day { day || weekday } else { day && weekday }
    }

    /// Premier déclenchement strictement après `after` (secondes Unix).
    fn next_after(&self, after: i64, tz: &TimeZone) -> Option<i64> {
        let start = Timestamp::from_second(after).ok()?.to_zoned(tz.clone());
        for offset in 0..=366 {
            let date = start.date().checked_add(jiff::Span::new().days(offset)).ok()?;
            if !self.matches_date(date) {
                continue;
            }
            for hour in (0..24).filter(|h| self.hours & (1 << h) != 0) {
                for minute in (0..60).filter(|m| self.minutes & (1 << m) != 0) {
                    let Ok(at) = date.at(hour as i8, minute as i8, 0, 0).to_zoned(tz.clone()) else {
                        continue;
                    };
                    let at = at.timestamp().as_second();
                    if at > after {
                        return Some(at);
                    }
                }
            }
        }
        None
    }
}

/// Bits des valeurs d'un champ : `*`, `5`, `1-5`, `*/15`, `10-40/10`, listes `1,3,5`.
fn field(text: &str, min: u32, max: u32) -> Option<u64> {
    let mut bits = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|&s| s > 0)?),
            None => (part, 1),
        };
        let (from, to) = match range {
            "*" => (min, max),
            _ => match range.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                // `5/10` : de 5 jusqu'au bout.
                None if step > 1 => (range.parse().ok()?, max),
                None => {
                    let v = range.parse().ok()?;
                    (v, v)
                }
            },
        };
        if from < min || to > max || from > to {
            return None;
        }
        for v in (from..=to).step_by(step as usize) {
            bits |= 1 << v;
        }
    }
    Some(bits)
}

fn describe_cron(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    let num = |s: &str| s.parse::<u32>().ok();
    let every = |s: &str| s.strip_prefix("*/").and_then(num);
    match fields[..] {
        [m, "*", "*", "*", "*"] if every(m).is_some() => format!("toutes les {} min", every(m).unwrap_or(0)),
        ["*", "*", "*", "*", "*"] => "chaque minute".into(),
        [m, "*", "*", "*", "*"] if num(m).is_some() => "toutes les heures".into(),
        [m, h, "*", "*", "*"] if num(m).is_some() && every(h).is_some() => {
            format!("toutes les {} h", every(h).unwrap_or(0))
        }
        [m, h, "*", "*", dow] if num(m).is_some() && num(h).is_some() => {
            let time = format!("{:02}:{:02}", num(h).unwrap_or(0), num(m).unwrap_or(0));
            match dow {
                "*" => format!("tous les jours à {time}"),
                "1-5" => format!("en semaine à {time}"),
                _ => match num(dow) {
                    Some(d) if d <= 7 => {
                        const DAYS: [&str; 7] =
                            ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"];
                        format!("le {} à {time}", DAYS[d as usize % 7])
                    }
                    _ => expr.to_owned(),
                },
            }
        }
        _ => expr.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn post(tool: &str, input: Value, response: Value) -> Value {
        json!({"hook_event_name": "PostToolUse", "tool_name": tool, "tool_input": input, "tool_response": response})
    }

    #[test]
    fn create_and_delete() {
        let mut s = Schedule::default();
        // Réponse réelle de Claude Code (`toolUseResult`).
        let created = post(
            "CronCreate",
            json!({"cron": "*/5 * * * *", "prompt": "vérifie la CI"}),
            json!({"id": "1df0eb1a", "humanSchedule": "*/5 * * * *", "recurring": true, "durable": false,
                   "nextFireAt": (NOW + 120) * 1000}),
        );
        assert!(s.apply_hook(&created, NOW));
        let tasks = s.active(NOW);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].describe(), "toutes les 5 min");
        assert_eq!(tasks[0].next_run(NOW), Some((NOW + 120, false)));

        assert_eq!(s.active(NOW)[0].cancel_prompt(), "Annule la tâche planifiée 1df0eb1a (CronDelete), sans rien faire d'autre.");
        assert!(s.mark_cancelling("1df0eb1a"));
        assert!(!s.mark_cancelling("inconnu"));
        // Claude n'a pas annulé : la demande peut être refaite.
        s.apply_hook(&json!({"hook_event_name": "Stop"}), NOW);
        assert!(!s.active(NOW)[0].cancelling);

        assert!(s.apply_hook(&post("CronDelete", json!({"id": "1df0eb1a"}), json!({"id": "1df0eb1a"})), NOW));
        assert!(s.is_empty());
    }

    #[test]
    fn text_response_and_one_shot_lifecycle() {
        let mut s = Schedule::default();
        let created = post(
            "CronCreate",
            json!({"cron": "30 14 12 11 *", "prompt": "relance le déploiement", "recurring": false}),
            json!("Scheduled one-shot task 9abc0001 (30 14 12 11 *). Next run: …"),
        );
        s.apply_hook(&created, NOW);
        let task = &s.active(NOW)[0];
        assert_eq!(task.id, "9abc0001");
        assert!(!task.is_recurring());
        assert_eq!(task.describe(), "une fois");

        // Le prompt de la tâche part : elle disparaît.
        s.apply_hook(&json!({"hook_event_name": "UserPromptSubmit", "prompt": "relance le déploiement"}), NOW + 60);
        assert!(s.is_empty());
    }

    #[test]
    fn expiry_and_session_restart() {
        let mut s = Schedule::default();
        s.apply_hook(&post("CronCreate", json!({"cron": "7 * * * *", "prompt": "p"}), json!({"id": "a"})), NOW);
        assert_eq!(s.active(NOW + RECURRING_LIFETIME - 1).len(), 1);
        assert!(s.active(NOW + RECURRING_LIFETIME).is_empty());

        // `/clear` garde le processus ; un redémarrage non.
        s.apply_hook(&json!({"hook_event_name": "SessionStart", "source": "clear"}), NOW);
        assert!(!s.is_empty());
        s.apply_hook(&json!({"hook_event_name": "SessionStart", "source": "resume"}), NOW);
        assert!(s.is_empty());
    }

    #[test]
    fn wakeup_replaces_and_stops() {
        let mut s = Schedule::default();
        let wake = |delay: u32| post("ScheduleWakeup", json!({"delaySeconds": delay, "reason": "surveille la CI"}), json!(null));
        s.apply_hook(&wake(10), NOW);
        let task = &s.active(NOW)[0];
        // Délai ramené au minimum du runtime.
        assert_eq!(task.next_run(NOW), Some((NOW + 60, false)));
        assert_eq!(task.summary(), "surveille la CI");

        s.apply_hook(&wake(1200), NOW);
        assert_eq!(s.active(NOW).len(), 1);
        s.apply_hook(&post("ScheduleWakeup", json!({"stop": true}), json!(null)), NOW);
        assert!(s.is_empty());
    }

    #[test]
    fn cron_next_run() {
        let tz = TimeZone::UTC;
        // 2027-01-15 10:02:00 UTC, un vendredi.
        let at = |s: &str| s.parse::<Timestamp>().unwrap().as_second();
        let now = at("2027-01-15T10:02:00Z");
        let next = |expr: &str| Cron::parse(expr).unwrap().next_after(now, &tz).unwrap();
        assert_eq!(next("*/5 * * * *"), at("2027-01-15T10:05:00Z"));
        assert_eq!(next("2 * * * *"), at("2027-01-15T11:02:00Z"));
        assert_eq!(next("3 9 * * 1-5"), at("2027-01-18T09:03:00Z"));
        assert_eq!(next("0 0 1 * *"), at("2027-02-01T00:00:00Z"));
        // Dimanche, écrit 7.
        assert_eq!(next("0 8 * * 7"), at("2027-01-17T08:00:00Z"));
        // Jour du mois OU jour de semaine.
        assert_eq!(next("0 8 20 * 1"), at("2027-01-18T08:00:00Z"));
        assert!(Cron::parse("*/0 * * * *").is_none());
        assert!(Cron::parse("MON * * * *").is_none());
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe_cron("57 8 * * *"), "tous les jours à 08:57");
        assert_eq!(describe_cron("3 9 * * 1-5"), "en semaine à 09:03");
        assert_eq!(describe_cron("7 */2 * * *"), "toutes les 2 h");
        assert_eq!(describe_cron("0 9 * * 1"), "le lundi à 09:00");
        assert_eq!(describe_cron("0 9 1 * *"), "0 9 1 * *");
    }
}
