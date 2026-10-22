//! Flux stream-json de `claude -p` (specs §10.1), réduit en fil de conversation.
//!
//! Format interne, non documenté en détail : chaque ligne est lue comme un
//! `serde_json::Value` et seuls les champs connus sont exploités. Une ligne
//! inconnue ou illisible est ignorée, jamais une erreur.
//!
//! Avec `--include-partial-messages`, un bloc (texte, réflexion, outil) arrive
//! d'abord en morceaux (`stream_event`), puis en entier dans un message
//! `assistant`. Le texte affiché vient des morceaux ; du message complet, on
//! ne garde alors que l'entrée des outils. Sans morceaux (transcript, ancienne
//! version), le message complet fait foi.

use std::collections::HashSet;

use serde_json::{Value, json};

/// Une entrée du fil.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User { text: String },
    Assistant { text: String },
    Thinking { text: String },
    Tool(ToolCall),
    /// Information de Claude Code : refus de permission, erreur, compactage…
    Notice { text: String, error: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Entrée complète, connue à la fin du bloc.
    pub input: Value,
    /// JSON de l'entrée reçu en morceaux pendant le streaming.
    partial_input: String,
    pub result: Option<ToolResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolCall {
    /// Appel décrit par une demande de permission (`tool_name`, `input`, `tool_use_id`).
    pub fn from_permission_request(v: &Value) -> Self {
        Self {
            id: string(v, "tool_use_id"),
            name: string(v, "tool_name"),
            input: v.get("input").cloned().unwrap_or(Value::Null),
            partial_input: String::new(),
            result: None,
        }
    }

    /// Résumé d'une ligne : la commande, le fichier ou le motif concerné.
    pub fn summary(&self) -> String {
        let field = |key: &str| self.input.get(key).and_then(Value::as_str);
        let text = field("command")
            .or_else(|| field("file_path"))
            .or_else(|| field("notebook_path"))
            .or_else(|| field("pattern"))
            .or_else(|| field("url"))
            .or_else(|| field("query"))
            .or_else(|| field("description"))
            .or_else(|| field("prompt"))
            .unwrap_or_default();
        text.lines().next().unwrap_or_default().to_owned()
    }
}

/// État d'une session pilotée en stream-json.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    pub entries: Vec<Entry>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// Un tour est en cours (prompt envoyé, pas encore de `result`).
    pub busy: bool,
    /// Coût cumulé rapporté par le dernier `result` (depuis le lancement du processus).
    pub cost_usd: Option<f64>,
    /// Tokens envoyés au modèle au dernier appel : occupation du contexte.
    pub context_tokens: Option<u64>,
    /// Entrée qui reçoit les morceaux du bloc en cours.
    streaming: Option<usize>,
    /// Messages reçus en morceaux : leur version complète ne doit pas être ajoutée une seconde fois.
    streamed_messages: HashSet<String>,
}

impl Conversation {
    /// Message à écrire sur stdin pour envoyer `text`, et entrée correspondante du fil.
    pub fn user_message(&mut self, text: &str) -> String {
        self.entries.push(Entry::User { text: text.to_owned() });
        self.busy = true;
        json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
            .to_string()
    }

    /// Applique une ligne de stdout. Renvoie `false` si elle n'a rien changé.
    pub fn apply_line(&mut self, line: &str) -> bool {
        match serde_json::from_str::<Value>(line.trim()) {
            Ok(value) if value.is_object() => self.apply(&value),
            // Bannière d'un alias, avertissement du shell… : ignorés.
            _ => false,
        }
    }

    pub fn apply(&mut self, v: &Value) -> bool {
        let str_field = |key: &str| v.get(key).and_then(Value::as_str);
        if let Some(id) = str_field("session_id") {
            self.session_id = Some(id.to_owned());
        }
        match str_field("type") {
            Some("system") => self.apply_system(v),
            Some("stream_event") => v.get("event").is_some_and(|e| self.apply_stream_event(e)),
            Some("assistant") => v.get("message").is_some_and(|m| self.apply_assistant(m)),
            Some("user") => v.get("message").is_some_and(|m| self.apply_user(m, v)),
            Some("result") => {
                self.busy = false;
                self.streaming = None;
                if let Some(cost) = v.get("total_cost_usd").and_then(Value::as_f64) {
                    self.cost_usd = Some(cost);
                }
                let failed = v.get("is_error").and_then(Value::as_bool) == Some(true)
                    || str_field("subtype").is_some_and(|s| s != "success");
                if failed {
                    let text = str_field("result")
                        .map(str::to_owned)
                        .or_else(|| {
                            let errors = v.get("errors")?.as_array()?;
                            Some(errors.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
                        })
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| "Le tour s'est terminé en erreur.".to_owned());
                    self.entries.push(Entry::Notice { text, error: true });
                }
                true
            }
            _ => false,
        }
    }

    fn apply_system(&mut self, v: &Value) -> bool {
        let str_field = |key: &str| v.get(key).and_then(Value::as_str);
        match str_field("subtype") {
            Some("init") => {
                self.model = str_field("model").map(str::to_owned);
                self.permission_mode = str_field("permissionMode").map(str::to_owned);
                true
            }
            Some("permission_denied") => {
                let tool = str_field("tool_name").unwrap_or("outil");
                let reason = str_field("message").or(str_field("decision_reason")).unwrap_or_default();
                self.entries.push(Entry::Notice { text: format!("{tool} refusé. {reason}").trim().to_owned(), error: true });
                true
            }
            Some("compact_boundary") => {
                self.entries.push(Entry::Notice { text: "Contexte compacté.".into(), error: false });
                true
            }
            _ => false,
        }
    }

    fn apply_stream_event(&mut self, event: &Value) -> bool {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(message) = event.get("message") {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        self.streamed_messages.insert(id.to_owned());
                    }
                    self.update_context(message.get("usage"));
                }
                false
            }
            Some("content_block_start") => {
                let Some(block) = event.get("content_block") else { return false };
                let entry = match block.get("type").and_then(Value::as_str) {
                    Some("text") => Entry::Assistant { text: string(block, "text") },
                    Some("thinking") => Entry::Thinking { text: string(block, "thinking") },
                    Some("tool_use") => Entry::Tool(ToolCall {
                        id: string(block, "id"),
                        name: string(block, "name"),
                        input: Value::Null,
                        partial_input: String::new(),
                        result: None,
                    }),
                    _ => return false,
                };
                self.entries.push(entry);
                self.streaming = Some(self.entries.len() - 1);
                true
            }
            Some("content_block_delta") => {
                let (Some(index), Some(delta)) = (self.streaming, event.get("delta")) else { return false };
                let piece = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
                match (self.entries.get_mut(index), delta.get("type").and_then(Value::as_str)) {
                    (Some(Entry::Assistant { text }), Some("text_delta")) => text.push_str(piece("text")),
                    (Some(Entry::Thinking { text }), Some("thinking_delta")) => text.push_str(piece("thinking")),
                    (Some(Entry::Tool(tool)), Some("input_json_delta")) => {
                        tool.partial_input.push_str(piece("partial_json"));
                        return false;
                    }
                    _ => return false,
                }
                true
            }
            Some("content_block_stop") => {
                if let Some(Entry::Tool(tool)) = self.streaming.and_then(|i| self.entries.get_mut(i))
                    && tool.input.is_null()
                {
                    tool.input = serde_json::from_str(&tool.partial_input).unwrap_or(Value::Null);
                }
                self.streaming = None;
                true
            }
            _ => false,
        }
    }

    fn apply_assistant(&mut self, message: &Value) -> bool {
        let streamed = message.get("id").and_then(Value::as_str).is_some_and(|id| self.streamed_messages.contains(id));
        if !streamed {
            self.update_context(message.get("usage"));
        }
        let Some(blocks) = message.get("content").and_then(Value::as_array) else { return false };
        let mut changed = false;
        for block in blocks {
            let kind = block.get("type").and_then(Value::as_str);
            if kind == Some("tool_use") {
                let id = string(block, "id");
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                if let Some(tool) = self.tool_mut(&id) {
                    tool.input = input;
                } else if !streamed {
                    self.entries.push(Entry::Tool(ToolCall {
                        id,
                        name: string(block, "name"),
                        input,
                        partial_input: String::new(),
                        result: None,
                    }));
                }
                changed = true;
                continue;
            }
            if streamed {
                continue;
            }
            match kind {
                Some("text") => self.entries.push(Entry::Assistant { text: string(block, "text") }),
                Some("thinking") => self.entries.push(Entry::Thinking { text: string(block, "thinking") }),
                _ => continue,
            }
            changed = true;
        }
        changed
    }

    /// Résultats d'outils ; dans un transcript, aussi les prompts de l'utilisateur.
    fn apply_user(&mut self, message: &Value, line: &Value) -> bool {
        let meta = line.get("isMeta").and_then(Value::as_bool) == Some(true);
        match message.get("content") {
            Some(Value::String(text)) if !meta && !is_command_output(text) => {
                self.entries.push(Entry::User { text: text.clone() });
                true
            }
            Some(Value::Array(blocks)) => {
                let mut changed = false;
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_result") => {
                            let id = string(block, "tool_use_id");
                            let result = ToolResult {
                                text: content_text(block.get("content")),
                                is_error: block.get("is_error").and_then(Value::as_bool) == Some(true),
                            };
                            if let Some(tool) = self.tool_mut(&id) {
                                tool.result = Some(result);
                                changed = true;
                            }
                        }
                        Some("text") if !meta => {
                            let text = string(block, "text");
                            // Nos propres prompts ne sont pas renvoyés en direct ; dans un
                            // transcript, ils arrivent sous cette forme.
                            if !is_command_output(&text)
                                && !matches!(self.entries.last(), Some(Entry::User { text: t }) if *t == text)
                            {
                                self.entries.push(Entry::User { text });
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
                changed
            }
            _ => false,
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    fn update_context(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage else { return };
        let tokens = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let total = tokens("input_tokens") + tokens("cache_creation_input_tokens") + tokens("cache_read_input_tokens");
        if total > 0 {
            self.context_tokens = Some(total);
        }
    }

    /// Place `earlier` (relu dans le transcript) avant les entrées déjà reçues.
    pub fn prepend_history(&mut self, earlier: Conversation) {
        let count = earlier.entries.len();
        self.entries.splice(0..0, earlier.entries);
        self.streaming = self.streaming.map(|i| i + count);
        self.session_id = self.session_id.take().or(earlier.session_id);
        self.model = self.model.take().or(earlier.model);
        self.context_tokens = self.context_tokens.or(earlier.context_tokens);
    }

    /// Fil d'une session passée, relu depuis son transcript (`--resume` ne le renvoie pas).
    pub fn from_transcript(text: &str) -> Self {
        let mut conversation = Self::default();
        for line in text.lines() {
            // Seuls les messages intéressent le fil ; le reste (titres, coûts…) est sauté sans parsing.
            if line.contains(r#""type":"user""#) || line.contains(r#""type":"assistant""#) {
                conversation.apply_line(line);
            }
        }
        conversation.busy = false;
        conversation.streaming = None;
        conversation
    }
}

fn string(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// Contenu d'un résultat d'outil : chaîne, ou liste de blocs texte.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Sorties de slash commands et rappels système enregistrés comme messages utilisateur.
fn is_command_output(text: &str) -> bool {
    let text = text.trim_start();
    ["<command-", "<local-command-", "<system-reminder>", "Caveat:"].iter().any(|p| text.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all(c: &mut Conversation, lines: &[Value]) {
        for line in lines {
            c.apply(line);
        }
    }

    fn event(event: Value) -> Value {
        json!({ "type": "stream_event", "event": event, "session_id": "s1" })
    }

    #[test]
    fn streamed_text_is_not_duplicated() {
        let mut c = Conversation::default();
        c.user_message("salut");
        apply_all(&mut c, &[
            json!({"type": "system", "subtype": "init", "session_id": "s1", "model": "claude-haiku-5-5", "permissionMode": "default"}),
            event(json!({"type": "message_start", "message": {"id": "m1", "usage": {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 900}}})),
            event(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
            event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Bon"}})),
            event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "jour"}})),
            json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "text", "text": "Bonjour"}]}}),
            event(json!({"type": "content_block_stop", "index": 0})),
            json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.01, "session_id": "s1"}),
        ]);
        assert_eq!(c.entries, vec![Entry::User { text: "salut".into() }, Entry::Assistant { text: "Bonjour".into() }]);
        assert_eq!((c.session_id.as_deref(), c.model.as_deref()), (Some("s1"), Some("claude-haiku-5-5")));
        assert_eq!((c.busy, c.cost_usd, c.context_tokens), (false, Some(0.01), Some(1002)));
    }

    #[test]
    fn tool_calls_get_input_and_result() {
        let mut c = Conversation::default();
        apply_all(&mut c, &[
            event(json!({"type": "message_start", "message": {"id": "m2"}})),
            event(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}})),
            event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"ls"}})),
            event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": " -la\"}"}})),
            event(json!({"type": "content_block_stop", "index": 0})),
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "total 0"}], "is_error": false}]}}),
        ]);
        let Entry::Tool(tool) = &c.entries[0] else { panic!("outil attendu") };
        assert_eq!((tool.name.as_str(), tool.summary()), ("Bash", "ls -la".to_owned()));
        assert_eq!(tool.result, Some(ToolResult { text: "total 0".into(), is_error: false }));
    }

    #[test]
    fn full_messages_without_streaming_and_errors() {
        let mut c = Conversation::default();
        apply_all(&mut c, &[
            json!({"type": "assistant", "message": {"id": "m3", "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/tmp/a.rs"}}
            ]}}),
            json!({"type": "system", "subtype": "permission_denied", "tool_name": "Bash", "message": "Interdit."}),
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "errors": ["boom"]}),
        ]);
        assert_eq!(c.entries[0], Entry::Thinking { text: "hmm".into() });
        assert!(matches!(&c.entries[1], Entry::Tool(t) if t.summary() == "/tmp/a.rs"));
        assert_eq!(c.entries[2], Entry::Notice { text: "Bash refusé. Interdit.".into(), error: true });
        assert_eq!(c.entries[3], Entry::Notice { text: "boom".into(), error: true });
    }

    #[test]
    fn unknown_and_garbage_lines_are_ignored() {
        let mut c = Conversation::default();
        assert!(!c.apply_line("╔═ HEADROOM WRAP ═╗"));
        assert!(!c.apply_line(r#"{"type": "rate_limit_event", "rate_limit_info": {}}"#));
        assert!(!c.apply_line(r#"{"type": "futur", "x": [1]}"#));
        assert!(c.entries.is_empty());
    }

    #[test]
    fn history_goes_before_live_entries() {
        let mut live = Conversation::default();
        live.apply(&event(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "a"}})));
        let earlier = Conversation::from_transcript(r#"{"type":"user","message":{"role":"user","content":"avant"}}"#);
        live.prepend_history(earlier);
        live.apply(&event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "b"}})));
        assert_eq!(live.entries, vec![Entry::User { text: "avant".into() }, Entry::Assistant { text: "ab".into() }]);
    }

    #[test]
    fn transcript_replay_keeps_prompts_and_skips_meta() {
        let transcript = [
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>x"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"Corrige le bug"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"C'est fait."}]}}"#,
            r#"{"type":"custom-title","customTitle":"Bug"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<command-name>/clear</command-name>"}]}}"#,
        ]
        .join("\n");
        let c = Conversation::from_transcript(&transcript);
        assert_eq!(c.entries, vec![
            Entry::User { text: "Corrige le bug".into() },
            Entry::Assistant { text: "C'est fait.".into() },
        ]);
    }
}
