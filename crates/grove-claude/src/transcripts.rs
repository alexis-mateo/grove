//! Sessions passées, lues depuis `~/.claude/projects/<cwd encodé>/<id>.jsonl`.
//!
//! Format interne de Claude Code : lecture défensive, lignes et champs
//! inconnus ignorés, aucune erreur bloquante.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// Lignes parcourues au maximum pour trouver le premier prompt.
const FIRST_PROMPT_LINES: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub modified: SystemTime,
    /// Titre de la session : celui donné par `/rename`, sinon celui généré par Claude.
    pub title: Option<String>,
    /// Premier prompt, sur une ligne.
    pub first_prompt: Option<String>,
    pub cost_usd: Option<f64>,
}

/// Dossier des transcripts de `cwd` : chaque caractère non alphanumérique devient `-`.
pub fn project_dir(cwd: &Path) -> Option<PathBuf> {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(crate::settings::claude_dir()?.join("projects").join(encoded))
}

/// Sessions de `cwd`, de la plus récente à la plus ancienne. Lit chaque
/// transcript en entier : à appeler hors du thread de l'UI.
pub fn list_sessions(cwd: &Path) -> Vec<SessionSummary> {
    let Some(dir) = project_dir(cwd) else { return Vec::new() };
    list_sessions_in(&dir)
}

pub fn list_sessions_in(dir: &Path) -> Vec<SessionSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut sessions: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_owned();
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some(summarize(id, path, modified))
        })
        .collect();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    sessions
}

fn summarize(id: String, path: PathBuf, modified: SystemTime) -> SessionSummary {
    let mut summary = SessionSummary { id, path, modified, title: None, first_prompt: None, cost_usd: None };
    let Ok(file) = File::open(&summary.path) else { return summary };

    let (mut custom, mut ai, mut legacy) = (None, None, None);
    for (index, line) in BufReader::new(file).split(b'\n').enumerate() {
        let Ok(line) = line else { break };
        let Ok(line) = std::str::from_utf8(&line) else { continue };
        // Les transcripts font plusieurs Mo : on ne parse que les lignes utiles.
        let wanted = |kind: &str| line.contains(&format!("\"type\":\"{kind}\""));
        let looking_for_prompt = summary.first_prompt.is_none() && index < FIRST_PROMPT_LINES;
        if !(wanted("custom-title")
            || wanted("ai-title")
            || wanted("summary")
            || wanted("cost-state")
            || (looking_for_prompt && wanted("user")))
        {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let text = |key: &str| entry.get(key).and_then(Value::as_str).map(one_line).filter(|t| !t.is_empty());
        match entry.get("type").and_then(Value::as_str) {
            // Plusieurs entrées possibles (renommages successifs) : la dernière gagne.
            Some("custom-title") => custom = text("customTitle").or(custom),
            Some("ai-title") => ai = text("aiTitle").or(ai),
            Some("summary") => legacy = text("summary").or(legacy),
            Some("cost-state") => {
                summary.cost_usd = entry.get("totalCostUSD").and_then(Value::as_f64).or(summary.cost_usd)
            }
            Some("user") if looking_for_prompt => {
                if entry.get("isMeta").and_then(Value::as_bool) != Some(true) {
                    summary.first_prompt = entry.pointer("/message/content").and_then(prompt_text);
                }
            }
            _ => {}
        }
    }
    summary.title = custom.or(ai).or(legacy);
    summary
}

/// Texte d'un message utilisateur. Une commande (`<command-name>/x</command-name>`) donne `/x`.
fn prompt_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    if let Some(command) = between(&text, "<command-name>", "</command-name>") {
        let args = between(&text, "<command-args>", "</command-args>").unwrap_or_default();
        return Some(one_line(&format!("{command} {args}")));
    }
    if text.trim_start().starts_with('<') {
        return None;
    }
    Some(one_line(&text)).filter(|t| !t.is_empty())
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = &text[text.find(start)? + start.len()..];
    Some(rest[..rest.find(end)?].trim())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_cwd_like_claude_code() {
        // SAFETY: test mono-thread sur une variable propre à ce test.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/c") };
        assert_eq!(
            project_dir(Path::new("/home/me/.config/my_app")).unwrap(),
            PathBuf::from("/c/projects/-home-me--config-my-app")
        );
    }

    #[test]
    fn titles_prompts_and_cost() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.jsonl"),
            [
                r#"{"type":"queue-operation"}"#,
                "pas du json",
                r#"{"type":"user","isMeta":true,"message":{"content":"caveat"}}"#,
                r#"{"type":"user","message":{"role":"user","content":"Corrige le bug\n du login"}}"#,
                r#"{"type":"ai-title","aiTitle":"Bug du login"}"#,
                r#"{"type":"cost-state","totalCostUSD":0.5}"#,
                r#"{"type":"custom-title","customTitle":"Login cassé"}"#,
                r#"{"type":"custom-title","customTitle":"Login réparé"}"#,
                r#"{"type":"cost-state","totalCostUSD":1.25}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.jsonl"),
            [
                r#"{"type":"user","message":{"content":"<command-message>x</command-message><command-name>/review</command-name><command-args>42</command-args>"}}"#,
                r#"{"type":"ai-title","aiTitle":"Revue de la PR 42"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(dir.path().join("c.jsonl"), r#"{"type":"user","message":{"content":"Salut"}}"#).unwrap();

        let mut sessions = list_sessions_in(dir.path());
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        let got: Vec<_> = sessions
            .iter()
            .map(|s| (s.id.as_str(), s.title.as_deref(), s.first_prompt.as_deref(), s.cost_usd))
            .collect();
        assert_eq!(
            got,
            [
                ("a", Some("Login réparé"), Some("Corrige le bug du login"), Some(1.25)),
                ("b", Some("Revue de la PR 42"), Some("/review 42"), None),
                ("c", None, Some("Salut"), None),
            ]
        );
    }
}
