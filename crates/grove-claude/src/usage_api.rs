//! Lecture de l'usage du compte via l'endpoint interne d'Anthropic, comme ccstatusline.
//!
//! **Hors des points d'intégration officiels** : endpoint non documenté, et
//! réutilisation du jeton OAuth de Claude Code (cf. specs §8.5). Désactivé par
//! défaut (`[claude] usage_api`). Le jeton est seulement lu : jamais rafraîchi,
//! jamais réécrit, envoyé uniquement à `api.anthropic.com`.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::session::{RateLimits, RateWindow};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TIMEOUT: Duration = Duration::from_secs(10);

/// Délai minimal entre deux appels (celui de ccstatusline).
pub const POLL_INTERVAL: Duration = Duration::from_secs(180);
/// Attentes successives après un refus pour excès d'appels, sans `Retry-After`.
pub const RATE_LIMIT_BACKOFF: [Duration; 4] =
    [Duration::from_secs(30), Duration::from_secs(60), Duration::from_secs(120), Duration::from_secs(300)];
/// On ne rappelle jamais plus vite que ça, même si `Retry-After` le permet.
const MIN_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum FetchError {
    /// Pas de jeton utilisable (connexion par clé d'API, jeton expiré…).
    NoToken,
    /// Trop d'appels : réessayer après `Retry-After`, s'il est fourni.
    RateLimited(Option<Duration>),
    Other(anyhow::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::NoToken => write!(f, "pas de jeton claude.ai valide"),
            FetchError::RateLimited(Some(after)) => write!(f, "trop d'appels (Retry-After : {} s)", after.as_secs()),
            FetchError::RateLimited(None) => write!(f, "trop d'appels (sans Retry-After)"),
            FetchError::Other(err) => write!(f, "{err:#}"),
        }
    }
}

/// Jeton d'accès de `<claude_dir>/.credentials.json`, s'il n'a pas expiré.
fn read_token(claude_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(claude_dir.join(".credentials.json")).ok()?;
    let credentials: Value = serde_json::from_slice(&bytes).ok()?;
    let oauth = credentials.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str().filter(|t| !t.is_empty())?;
    // `expiresAt` en millisecondes. Claude Code le rafraîchit lui-même : on attend.
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_i64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
        if expires_at <= now {
            return None;
        }
    }
    Some(token.to_owned())
}

pub fn fetch() -> Result<RateLimits, FetchError> {
    let claude_dir = crate::settings::claude_dir().ok_or(FetchError::NoToken)?;
    let token = read_token(&claude_dir).ok_or(FetchError::NoToken)?;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "grove")
        .call()
        .map_err(|err| FetchError::Other(err.into()))?;

    let status = response.status().as_u16();
    if status == 429 {
        let after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok())
            .map(|secs| Duration::from_secs(secs).max(MIN_RETRY));
        return Err(FetchError::RateLimited(after));
    }
    if status == 401 || status == 403 {
        return Err(FetchError::NoToken);
    }
    let body = response.body_mut().read_to_string().map_err(|err| FetchError::Other(err.into()))?;
    if !(200..300).contains(&status) {
        return Err(FetchError::Other(anyhow::anyhow!("HTTP {status}")));
    }
    let value: Value = serde_json::from_str(&body).map_err(|err| FetchError::Other(err.into()))?;
    parse_response(&value).map_err(FetchError::Other)
}

/// Fenêtres `{ utilization, resets_at }` ; à défaut, la liste `limits[]` (`kind`, `percent`).
pub fn parse_response(v: &Value) -> Result<RateLimits> {
    let window = |key: &str, kind: &str| -> Option<RateWindow> {
        let from_bucket = v.get(key).filter(|b| b.is_object()).and_then(|b| {
            Some(RateWindow {
                used_percent: b.get("utilization").and_then(Value::as_f64)?,
                resets_at: b.get("resets_at").and_then(Value::as_str).and_then(parse_time),
            })
        });
        from_bucket.or_else(|| {
            let limit = v.get("limits")?.as_array()?.iter().find(|l| l.get("kind").and_then(Value::as_str) == Some(kind))?;
            Some(RateWindow {
                used_percent: limit.get("percent").and_then(Value::as_f64)?,
                resets_at: limit.get("resets_at").and_then(Value::as_str).and_then(parse_time),
            })
        })
    };
    let limits = RateLimits {
        five_hour: window("five_hour", "session"),
        seven_day: window("seven_day", "weekly_all"),
        seven_day_opus: window("seven_day_opus", ""),
        seven_day_sonnet: window("seven_day_sonnet", ""),
    };
    if limits == RateLimits::default() {
        bail!("réponse sans limites d'usage");
    }
    Ok(limits)
}

fn parse_time(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Appel réel : `cargo test -p grove-claude live_fetch -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_fetch() {
        match fetch() {
            Ok(limits) => println!("{limits:?}"),
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn parses_buckets() {
        let limits = parse_response(&json!({
            "five_hour": {"utilization": 44.0, "resets_at": "2026-10-21T15:00:00.341087+00:00"},
            "seven_day": {"utilization": 20, "resets_at": "2026-10-23T20:00:00+00:00"},
            "seven_day_opus": null,
            "seven_day_sonnet": {"utilization": 0, "resets_at": null},
            "extra_usage": {"is_enabled": false}
        }))
        .unwrap();
        let five = limits.five_hour.unwrap();
        assert_eq!(five.used_percent, 44.0);
        assert_eq!(five.resets_at, Some(1_792_594_800));
        assert_eq!(limits.seven_day.unwrap().used_percent, 20.0);
        assert_eq!(limits.seven_day_opus, None);
        assert_eq!(limits.seven_day_sonnet, Some(RateWindow { used_percent: 0.0, resets_at: None }));
    }

    #[test]
    fn falls_back_to_limits_list() {
        let limits = parse_response(&json!({
            "limits": [
                {"kind": "session", "percent": 12.5, "resets_at": "2026-10-21T15:00:00Z"},
                {"kind": "weekly_all", "percent": 30}
            ]
        }))
        .unwrap();
        assert_eq!(limits.five_hour.unwrap().used_percent, 12.5);
        assert_eq!(limits.seven_day.unwrap().resets_at, None);
        assert!(parse_response(&json!({"error": "x"})).is_err());
    }

    #[test]
    fn expired_or_missing_token_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_token(dir.path()), None);
        let write = |expires: i64| {
            std::fs::write(
                dir.path().join(".credentials.json"),
                json!({"claudeAiOauth": {"accessToken": "tok", "expiresAt": expires}}).to_string(),
            )
            .unwrap();
        };
        write(1);
        assert_eq!(read_token(dir.path()), None);
        write(i64::MAX);
        assert_eq!(read_token(dir.path()).as_deref(), Some("tok"));
    }
}
