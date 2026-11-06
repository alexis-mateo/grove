//! Usage passé de Claude Code, par jour et par dossier, lu dans les transcripts
//! (`~/.claude/projects/**/*.jsonl`) : tokens de chaque réponse de l'API et
//! coût estimé au tarif public de l'API.
//!
//! Les transcripts ne donnent le coût qu'au niveau de la session (`cost-state`,
//! pas toujours présent) : on le recalcule réponse par réponse pour pouvoir le
//! ranger par jour. Avec un abonnement claude.ai, ce n'est pas ce qui est
//! facturé, mais un ordre de grandeur comparable d'un jour à l'autre.
//!
//! Lecture défensive (format interne) et incrémentale : les transcripts ne font
//! que grossir, chaque relecture ne lit que les lignes ajoutées.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::Value;

/// Profondeur maximale parcourue sous `projects` (`<dossier>/<session>/subagents/x.jsonl`).
const MAX_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Écritures dans le cache de prompt (5 min et 1 h).
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }
}

/// Une réponse de l'API.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// Jour local de la réponse.
    pub day: Date,
    /// Dossier de la session.
    pub cwd: String,
    pub model: String,
    pub tokens: Tokens,
    /// `None` : modèle dont le tarif n'est pas connu.
    pub cost_usd: Option<f64>,
}

/// Réponse lue, avec son identifiant : une même réponse apparaît sur une ligne
/// par bloc de contenu, et se retrouve dans plusieurs fichiers après une reprise.
#[derive(Clone, Debug)]
struct Entry {
    key: Option<String>,
    record: Record,
}

struct FileState {
    len: u64,
    modified: SystemTime,
    /// Fin de la dernière ligne complète lue.
    offset: u64,
    entries: Vec<Entry>,
}

/// Lecteur incrémental des transcripts. À garder d'une lecture à l'autre.
pub struct Scanner {
    tz: TimeZone,
    files: HashMap<PathBuf, FileState>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::with_time_zone(TimeZone::system())
    }
}

impl Scanner {
    pub fn with_time_zone(tz: TimeZone) -> Self {
        Self { tz, files: HashMap::new() }
    }

    /// Réponses de tous les transcripts de Claude Code. À appeler hors du thread de l'UI.
    pub fn scan(&mut self) -> Vec<Record> {
        match crate::settings::claude_dir() {
            Some(dir) => self.scan_dir(&dir.join("projects")),
            None => Vec::new(),
        }
    }

    pub fn scan_dir(&mut self, root: &Path) -> Vec<Record> {
        let mut paths = Vec::new();
        collect_transcripts(root, 0, &mut paths);
        self.files.retain(|path, _| paths.contains(path));
        for path in paths {
            self.read_file(path);
        }

        // Une réponse par identifiant : la ligne qui compte le plus de tokens de
        // sortie (les premières lignes d'une réponse en streaming en ont moins).
        let mut records: Vec<Record> = Vec::new();
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for entry in self.files.values().flat_map(|f| &f.entries) {
            match entry.key.as_deref() {
                Some(key) => match seen.get(key) {
                    Some(&index) => {
                        if entry.record.tokens.output > records[index].tokens.output {
                            records[index] = entry.record.clone();
                        }
                    }
                    None => {
                        seen.insert(key, records.len());
                        records.push(entry.record.clone());
                    }
                },
                None => records.push(entry.record.clone()),
            }
        }
        records
    }

    fn read_file(&mut self, path: PathBuf) {
        let Ok(meta) = std::fs::metadata(&path) else { return };
        let (len, modified) = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        let state = self.files.entry(path.clone()).or_insert(FileState { len: 0, modified, offset: 0, entries: Vec::new() });
        if state.offset > 0 && state.len == len && state.modified == modified {
            return;
        }
        // Fichier réécrit plus court : on repart du début.
        if len < state.offset {
            state.offset = 0;
            state.entries.clear();
        }
        state.len = len;
        state.modified = modified;
        let Ok(mut file) = File::open(&path) else { return };
        if file.seek(SeekFrom::Start(state.offset)).is_err() {
            return;
        }
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Ligne en cours d'écriture : relue la prochaine fois.
                    if line.last() != Some(&b'\n') {
                        break;
                    }
                    state.offset += n as u64;
                    if let Some(entry) = parse_line(&line, &self.tz) {
                        state.entries.push(entry);
                    }
                }
            }
        }
    }
}

fn collect_transcripts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && depth + 1 < MAX_DEPTH {
            collect_transcripts(&path, depth + 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

fn parse_line(line: &[u8], tz: &TimeZone) -> Option<Entry> {
    let line = std::str::from_utf8(line).ok()?;
    // Les transcripts pèsent des centaines de Mo : on ne parse que les réponses.
    if !line.contains("\"type\":\"assistant\"") || !line.contains("\"usage\"") {
        return None;
    }
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = entry.get("message")?;
    let model = message.get("model").and_then(Value::as_str)?;
    // Réponses produites par Claude Code lui-même (erreurs, interruptions).
    if model.starts_with('<') {
        return None;
    }
    let usage = message.get("usage")?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cache_write = count("cache_creation_input_tokens");
    let one_hour = usage.pointer("/cache_creation/ephemeral_1h_input_tokens").and_then(Value::as_u64).unwrap_or(0).min(cache_write);
    let tokens = Tokens {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_write,
        cache_read: count("cache_read_input_tokens"),
    };
    if tokens.total() == 0 {
        return None;
    }
    let fast = usage.get("speed").and_then(Value::as_str) == Some("fast");
    let timestamp: jiff::Timestamp = entry.get("timestamp").and_then(Value::as_str)?.parse().ok()?;
    let key = message
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| entry.get("requestId").and_then(Value::as_str))
        .map(str::to_owned);
    Some(Entry {
        key,
        record: Record {
            day: timestamp.to_zoned(tz.clone()).date(),
            cwd: entry.get("cwd").and_then(Value::as_str).unwrap_or_default().to_owned(),
            model: normalize_model(model).to_owned(),
            cost_usd: cost(model, &tokens, one_hour, fast),
            tokens,
        },
    })
}

/// `claude-opus-4-5-20251101` → `claude-opus-4-5` ; `claude-opus-5-5[1m]` → `claude-opus-5-5`.
fn normalize_model(model: &str) -> &str {
    let model = model.split('[').next().unwrap_or(model);
    match model.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => model,
    }
}

/// Tarif de l'API en dollars par million de tokens : entrée, sortie, lecture du cache.
/// Les écritures du cache coûtent 1,25× l'entrée (5 min) ou 2× (1 h).
/// Source : grille de prix publique d'Anthropic (novembre 2026).
fn price(model: &str) -> Option<(f64, f64, f64)> {
    Some(match model {
        "claude-fable-5-1" | "claude-mythos-5-1" => (10.0, 50.0, 0.25),
        "claude-fable-5" | "claude-mythos-5" => (10.0, 50.0, 1.0),
        "claude-opus-5-5" => (4.0, 20.0, 0.20),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" | "claude-opus-4-5" => (5.0, 25.0, 0.50),
        "claude-sonnet-5-5" | "claude-sonnet-5" => (2.0, 10.0, 0.20),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" => (3.0, 15.0, 0.30),
        "claude-haiku-5-5" => (0.10, 0.50, 0.01),
        "claude-haiku-4-5" => (1.0, 5.0, 0.10),
        _ => return None,
    })
}

/// Coût estimé d'une réponse. Le mode rapide n'a de tarif connu que sur Opus 5 et 5.5 (2×).
fn cost(model: &str, tokens: &Tokens, one_hour_writes: u64, fast: bool) -> Option<f64> {
    let model = normalize_model(model);
    let (mut input, mut output, mut read) = price(model)?;
    // Haiku 5.5 : tarif plus élevé au-delà de 100 000 tokens de prompt.
    if model == "claude-haiku-5-5" && tokens.input + tokens.cache_write + tokens.cache_read > 100_000 {
        (input, output, read) = (0.50, 2.50, 0.05);
    }
    let multiplier = match (fast, model) {
        (false, _) => 1.0,
        (true, "claude-opus-5" | "claude-opus-5-5") => 2.0,
        (true, _) => return None,
    };
    let five_minutes = tokens.cache_write - one_hour_writes;
    let dollars = tokens.input as f64 * input
        + tokens.output as f64 * output
        + tokens.cache_read as f64 * read
        + five_minutes as f64 * input * 1.25
        + one_hour_writes as f64 * input * 2.0;
    Some(dollars * multiplier / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, model: &str, at: &str, cwd: &str, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"{cwd}","requestId":"r-{id}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":1000,"output_tokens":{output},"cache_creation_input_tokens":2000,"cache_read_input_tokens":10000,"cache_creation":{{"ephemeral_1h_input_tokens":2000,"ephemeral_5m_input_tokens":0}}}}}}}}"#
        )
    }

    /// Sur les vrais transcripts : `cargo test -p grove-claude live_scan -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_scan() {
        let start = std::time::Instant::now();
        let mut scanner = Scanner::default();
        let records = scanner.scan();
        let first = start.elapsed();
        let start = std::time::Instant::now();
        let again = scanner.scan().len();
        println!("{} réponses en {first:?}, relecture {again} en {:?}", records.len(), start.elapsed());
        let mut days: std::collections::BTreeMap<Date, (f64, u64, u64)> = Default::default();
        for r in &records {
            let day = days.entry(r.day).or_default();
            day.0 += r.cost_usd.unwrap_or(0.0);
            day.1 += r.tokens.total();
            day.2 += u64::from(r.cost_usd.is_none());
        }
        for (day, (cost, tokens, unpriced)) in days.iter().rev().take(10) {
            println!("{day} {cost:>8.2} $ {tokens:>12} tokens, {unpriced} sans tarif");
        }
        let mut models: Vec<_> = records.iter().filter(|r| r.cost_usd.is_none()).map(|r| r.model.as_str()).collect();
        models.sort();
        models.dedup();
        println!("modèles sans tarif : {models:?}");
    }

    #[test]
    fn prices_follow_the_api_grid() {
        let tokens = Tokens { input: 1_000_000, output: 1_000_000, cache_write: 1_000_000, cache_read: 1_000_000 };
        // Opus 5.5 : 4 + 20 + 0,20 + 1 Mtok écrit pour 1 h (2 × 4).
        assert_eq!(cost("claude-opus-5-5", &tokens, 1_000_000, false), Some(4.0 + 20.0 + 0.2 + 8.0));
        // Écriture 5 min : 1,25 × 3 pour Sonnet 4.5 (identifiant daté).
        assert_eq!(cost("claude-sonnet-4-5-20250929", &tokens, 0, false), Some(3.0 + 15.0 + 0.3 + 3.75));
        assert_eq!(cost("claude-opus-5-5[1m]", &Tokens { output: 1_000_000, ..Default::default() }, 0, true), Some(40.0));
        assert_eq!(cost("claude-opus-4-8", &tokens, 0, true), None, "mode rapide sans tarif connu");
        assert_eq!(cost("claude-inconnu-9", &tokens, 0, false), None);
        let small = Tokens { input: 1_000_000, ..Default::default() };
        assert_eq!(cost("claude-haiku-5-5", &small, 0, false), Some(0.5), "au-delà de 100 000 tokens de prompt");
    }

    #[test]
    fn scans_dedups_and_reads_only_appended_lines() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-home-me-app");
        std::fs::create_dir_all(project.join("s1/subagents")).unwrap();
        let main = project.join("s1.jsonl");
        std::fs::write(
            &main,
            [
                r#"{"type":"user","message":{"content":"salut"}}"#.to_owned(),
                // Deux blocs de la même réponse : la seconde ligne a le compte final.
                line("m1", "claude-opus-5-5", "2026-11-04T10:00:00Z", "/home/me/app", 10),
                line("m1", "claude-opus-5-5", "2026-11-04T10:00:01Z", "/home/me/app", 500),
                line("m2", "<synthetic>", "2026-11-04T10:01:00Z", "/home/me/app", 5),
                String::new(),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(project.join("s1/subagents/agent-a.jsonl"), line("m3", "claude-haiku-5-5", "2026-11-05T23:30:00Z", "/home/me/app", 50) + "\n").unwrap();

        let mut scanner = Scanner::with_time_zone(TimeZone::UTC);
        let mut records = scanner.scan_dir(dir.path());
        records.sort_by(|a, b| a.model.cmp(&b.model));
        let got: Vec<_> = records.iter().map(|r| (r.model.as_str(), r.day.to_string(), r.tokens.output)).collect();
        assert_eq!(got, [("claude-haiku-5-5", "2026-11-05".into(), 50), ("claude-opus-5-5", "2026-11-04".into(), 500)]);
        assert_eq!(records[1].cwd, "/home/me/app");
        assert!(records[1].cost_usd.unwrap() > 0.0);

        // Une ligne ajoutée, une autre encore incomplète, et une reprise qui recopie m1.
        let mut appended = std::fs::read_to_string(&main).unwrap();
        appended.push_str(&line("m4", "claude-sonnet-5-5", "2026-11-05T09:00:00Z", "/home/me/app", 7));
        appended.push('\n');
        appended.push_str(r#"{"type":"assistant","#);
        std::fs::write(&main, appended).unwrap();
        std::fs::write(project.join("s2.jsonl"), line("m1", "claude-opus-5-5", "2026-11-04T10:00:01Z", "/home/me/app", 500) + "\n").unwrap();
        let records = scanner.scan_dir(dir.path());
        assert_eq!(records.len(), 3);
        assert!(records.iter().any(|r| r.model == "claude-sonnet-5-5"));
    }
}
