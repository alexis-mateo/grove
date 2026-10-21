use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{Message, MessageKind};

#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeStatus {
    /// Lancé, `SessionStart` pas encore reçu.
    Starting,
    Working { current_tool: Option<String> },
    NeedsInput { reason: NeedsInputReason },
    /// Tour terminé, en attente d'un nouveau prompt.
    Idle,
    Exited,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NeedsInputReason {
    /// Demande de permission pour un outil.
    Permission { tool: Option<String> },
    /// Claude attend une réponse depuis un moment.
    Idle,
    /// Autre notification, avec son message.
    Other(String),
}

impl NeedsInputReason {
    pub fn describe(&self) -> String {
        match self {
            NeedsInputReason::Permission { tool: Some(tool) } => format!("demande la permission : {tool}"),
            NeedsInputReason::Permission { tool: None } => "demande une permission".into(),
            NeedsInputReason::Idle => "attend ta réponse".into(),
            NeedsInputReason::Other(message) => message.clone(),
        }
    }
}

/// Métriques issues de la statusline. Tous les champs sont optionnels :
/// le format évolue avec les versions de Claude Code.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionMetrics {
    pub model: Option<String>,
    /// Niveau d'effort (`low`, `medium`, `high`…).
    pub effort: Option<String>,
    pub cost_usd: Option<f64>,
    /// Utilisation de la fenêtre de contexte, en pourcentage.
    pub context_percent: Option<f64>,
    pub context_tokens: Option<u64>,
    pub context_size: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

impl SessionMetrics {
    pub fn from_statusline(v: &Value) -> Self {
        let model = v
            .pointer("/model/display_name")
            .or_else(|| v.pointer("/model/id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let context = v.get("context_window");
        let context_size = context.and_then(|c| c.get("context_window_size")).and_then(Value::as_u64);
        let context_tokens = context.and_then(|c| c.get("current_usage")).map(|usage| {
            ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                .iter()
                .filter_map(|k| usage.get(k).and_then(Value::as_u64))
                .sum()
        });
        let context_percent = context
            .and_then(|c| c.get("used_percentage"))
            .and_then(Value::as_f64)
            .or_else(|| match (context_tokens, context_size) {
                (Some(tokens), Some(size)) if size > 0 => Some(tokens as f64 * 100.0 / size as f64),
                _ => None,
            });
        Self {
            model,
            effort: v.pointer("/effort/level").and_then(Value::as_str).map(str::to_owned),
            cost_usd: v.pointer("/cost/total_cost_usd").and_then(Value::as_f64),
            context_percent,
            context_tokens,
            context_size,
            lines_added: v.pointer("/cost/total_lines_added").and_then(Value::as_u64),
            lines_removed: v.pointer("/cost/total_lines_removed").and_then(Value::as_u64),
        }
    }
}

/// Limites d'usage du compte (abonnement claude.ai), communes à toutes les sessions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimits {
    /// Fenêtre glissante de 5 h.
    pub five_hour: Option<RateWindow>,
    /// Semaine, tous modèles confondus.
    pub seven_day: Option<RateWindow>,
    pub seven_day_opus: Option<RateWindow>,
    pub seven_day_sonnet: Option<RateWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateWindow {
    pub used_percent: f64,
    /// Date de remise à zéro, en secondes depuis l'epoch Unix.
    pub resets_at: Option<i64>,
}

impl RateLimits {
    /// `None` si la statusline ne transmet pas de limites (connexion par clé d'API, version ancienne).
    pub fn from_statusline(v: &Value) -> Option<Self> {
        let limits = v.get("rate_limits").filter(|l| l.is_object())?;
        let window = |key: &str| {
            let w = limits.get(key)?;
            Some(RateWindow {
                used_percent: w.get("used_percentage").and_then(Value::as_f64)?,
                resets_at: w.get("resets_at").and_then(Value::as_f64).map(|t| t as i64),
            })
        };
        let limits = Self {
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
            seven_day_opus: window("seven_day_opus"),
            seven_day_sonnet: window("seven_day_sonnet"),
        };
        (limits != Self::default()).then_some(limits)
    }
}

/// Changement d'état qui mérite une notification.
#[derive(Clone, Debug, PartialEq)]
pub enum Transition {
    /// La session attend l'utilisateur (permission, question…).
    NeedsInput(NeedsInputReason),
    /// Le tour est terminé.
    TurnFinished,
}

/// État d'une session Claude, alimenté par les messages du socket.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub session_id: Option<String>,
    pub status: ClaudeStatus,
    pub metrics: SessionMetrics,
}

impl Default for Session {
    fn default() -> Self {
        Self { session_id: None, status: ClaudeStatus::Starting, metrics: SessionMetrics::default() }
    }
}

impl Session {
    /// Applique un message. Renvoie une transition à notifier, le cas échéant.
    pub fn apply(&mut self, message: &Message) -> Option<Transition> {
        let payload = &message.payload;
        if let Some(id) = payload.get("session_id").and_then(Value::as_str) {
            self.session_id = Some(id.to_owned());
        }
        if message.kind == MessageKind::Statusline {
            self.metrics = SessionMetrics::from_statusline(payload);
            return None;
        }

        let str_field = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
        let event = str_field("hook_event_name")?;
        let (status, transition) = match event.as_str() {
            "SessionStart" => (ClaudeStatus::Idle, None),
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PostCompact" => {
                (ClaudeStatus::Working { current_tool: None }, None)
            }
            "PreToolUse" => (ClaudeStatus::Working { current_tool: str_field("tool_name") }, None),
            "PermissionRequest" => {
                let reason = NeedsInputReason::Permission { tool: str_field("tool_name") };
                (ClaudeStatus::NeedsInput { reason: reason.clone() }, Some(Transition::NeedsInput(reason)))
            }
            "Notification" => {
                let reason = match str_field("notification_type").as_deref() {
                    Some("permission_prompt") => NeedsInputReason::Permission { tool: None },
                    Some("idle_prompt") => NeedsInputReason::Idle,
                    _ => match str_field("message") {
                        Some(message) if message.contains("permission") => {
                            NeedsInputReason::Permission { tool: None }
                        }
                        Some(message) if message.contains("waiting for your input") => NeedsInputReason::Idle,
                        Some(message) => NeedsInputReason::Other(message),
                        None => NeedsInputReason::Idle,
                    },
                };
                // `PermissionRequest` a souvent déjà signalé la même attente.
                let already = matches!(
                    (&self.status, &reason),
                    (ClaudeStatus::NeedsInput { reason: NeedsInputReason::Permission { .. } },
                     NeedsInputReason::Permission { .. })
                );
                let transition = (!already).then(|| Transition::NeedsInput(reason.clone()));
                (ClaudeStatus::NeedsInput { reason }, transition)
            }
            "Stop" | "StopFailure" => (ClaudeStatus::Idle, Some(Transition::TurnFinished)),
            "SessionEnd" => (ClaudeStatus::Exited, None),
            _ => return None,
        };
        self.status = status;
        transition
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn hook(payload: Value) -> Message {
        Message { tab: "t".into(), kind: MessageKind::Hook, payload }
    }

    #[test]
    fn hook_sequence() {
        let mut s = Session::default();
        assert_eq!(s.apply(&hook(json!({"hook_event_name": "SessionStart", "session_id": "abc"}))), None);
        assert_eq!(s.session_id.as_deref(), Some("abc"));
        assert_eq!(s.status, ClaudeStatus::Idle);

        s.apply(&hook(json!({"hook_event_name": "UserPromptSubmit", "prompt": "salut"})));
        s.apply(&hook(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {}})));
        assert_eq!(s.status, ClaudeStatus::Working { current_tool: Some("Bash".into()) });

        let t = s.apply(&hook(json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"})));
        assert_eq!(t, Some(Transition::NeedsInput(NeedsInputReason::Permission { tool: Some("Bash".into()) })));
        // La notification de permission qui suit ne renotifie pas.
        let t = s.apply(&hook(json!({"hook_event_name": "Notification", "notification_type": "permission_prompt",
                                     "message": "Claude needs your permission to use Bash"})));
        assert_eq!(t, None);

        s.apply(&hook(json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"})));
        assert_eq!(s.status, ClaudeStatus::Working { current_tool: None });
        assert_eq!(s.apply(&hook(json!({"hook_event_name": "Stop"}))), Some(Transition::TurnFinished));
        assert_eq!(s.status, ClaudeStatus::Idle);

        s.apply(&hook(json!({"hook_event_name": "SessionEnd", "reason": "exit"})));
        assert_eq!(s.status, ClaudeStatus::Exited);
    }

    #[test]
    fn unknown_events_are_ignored() {
        let mut s = Session::default();
        assert_eq!(s.apply(&hook(json!({"hook_event_name": "FutureEvent", "x": 1}))), None);
        assert_eq!(s.apply(&hook(json!({"no_event": true}))), None);
        assert_eq!(s.status, ClaudeStatus::Starting);
    }

    #[test]
    fn rate_limits_and_effort() {
        let v = json!({
            "effort": {"level": "high"},
            "rate_limits": {
                "five_hour": {"used_percentage": 23.5, "resets_at": 1_761_000_000},
                "seven_day": {"used_percentage": 41, "resets_at": 1_761_500_000},
                "seven_day_opus": null
            }
        });
        assert_eq!(SessionMetrics::from_statusline(&v).effort.as_deref(), Some("high"));
        let limits = RateLimits::from_statusline(&v).unwrap();
        assert_eq!(limits.five_hour, Some(RateWindow { used_percent: 23.5, resets_at: Some(1_761_000_000) }));
        assert_eq!(limits.seven_day.unwrap().used_percent, 41.0);
        assert_eq!(limits.seven_day_opus, None);
        assert_eq!(RateLimits::from_statusline(&json!({"rate_limits": null})), None);
    }

    #[test]
    fn statusline_metrics() {
        let mut s = Session::default();
        s.apply(&Message {
            tab: "t".into(),
            kind: MessageKind::Statusline,
            payload: json!({
                "session_id": "abc",
                "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
                "cost": {"total_cost_usd": 0.42, "total_lines_added": 10, "total_lines_removed": 2},
                "context_window": {
                    "context_window_size": 200000,
                    "current_usage": {"input_tokens": 1000, "cache_read_input_tokens": 49000, "output_tokens": 5}
                },
                "champ_inconnu": [1, 2, 3]
            }),
        });
        let m = &s.metrics;
        assert_eq!(m.model.as_deref(), Some("Opus 5.5"));
        assert_eq!(m.effort, None);
        assert_eq!(m.cost_usd, Some(0.42));
        assert_eq!(m.context_tokens, Some(50_000));
        assert_eq!(m.context_percent, Some(25.0));
    }
}
