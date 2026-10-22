//! Demandes de permission de la vue native (specs §10.2).
//!
//! `claude -p --permission-prompt-tool mcp__grove__permission` consulte un
//! outil MCP avant chaque action soumise à permission. Cet outil est servi
//! par `grove mcp-permission` (serveur MCP sur stdio, lancé par Claude Code) :
//! il transmet la demande au socket de l'application et attend la décision
//! de l'utilisateur sur la même connexion.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::protocol::{ENV_SOCKET, ENV_TAB_ID, Message, MessageKind};

const SERVER: &str = "grove";
const TOOL: &str = "permission";

/// Nom complet de l'outil, pour `--permission-prompt-tool`.
pub fn permission_tool() -> String {
    format!("mcp__{SERVER}__{TOOL}")
}

/// Configuration MCP (`--mcp-config`) du serveur de permissions de l'onglet `tab`.
pub fn mcp_config(grove: &Path, socket: &Path, tab: &str) -> String {
    json!({
        "mcpServers": {
            SERVER: {
                "type": "stdio",
                "command": grove,
                "args": ["mcp-permission"],
                "env": { ENV_SOCKET: socket, ENV_TAB_ID: tab },
            }
        }
    })
    .to_string()
}

/// Réponse attendue par Claude Code : autoriser (avec l'entrée inchangée) ou refuser.
pub fn decision(allow: bool, input: &Value) -> Value {
    if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "Refusé par l'utilisateur dans Grove." })
    }
}

/// `grove mcp-permission` : boucle JSON-RPC sur stdin / stdout.
pub fn run_mcp_server() -> ExitCode {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(response) = handle(&request, ask_grove) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
    ExitCode::SUCCESS
}

/// Réponse à une requête JSON-RPC ; `None` pour une notification.
fn handle(request: &Value, ask: impl FnOnce(&Value) -> Value) -> Option<Value> {
    let id = request.get("id")?.clone();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result = match request.get("method").and_then(Value::as_str) {
        Some("initialize") => json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER, "version": env!("CARGO_PKG_VERSION") },
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({ "tools": [{
            "name": TOOL,
            "description": "Demande à l'utilisateur, dans Grove, s'il autorise l'appel d'outil.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string" },
                    "input": { "type": "object" },
                    "tool_use_id": { "type": "string" },
                },
                "required": ["tool_name", "input"],
            },
        }]}),
        Some("tools/call") if params.get("name").and_then(Value::as_str) == Some(TOOL) => {
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            let decision = ask(&arguments);
            json!({ "content": [{ "type": "text", "text": decision.to_string() }] })
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": "méthode inconnue" },
            }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Transmet la demande à Grove et attend la décision. Grove injoignable : refus.
fn ask_grove(arguments: &Value) -> Value {
    let input = arguments.get("input").cloned().unwrap_or(Value::Null);
    let refused = |reason: &str| json!({ "behavior": "deny", "message": reason });
    let (Ok(socket), Ok(tab)) = (std::env::var(ENV_SOCKET), std::env::var(ENV_TAB_ID)) else {
        return refused("Grove n'est pas joignable (variables d'environnement absentes).");
    };
    let message = Message { tab, kind: MessageKind::Permission, payload: arguments.clone() };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = UnixStream::connect(socket)?;
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        // Pas de délai : l'utilisateur prend le temps qu'il lui faut.
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        Ok(reply)
    })();
    match reply.ok().and_then(|r| serde_json::from_str::<Value>(&r).ok()) {
        Some(decision) if decision.get("behavior").is_some() => decision,
        _ => {
            log::debug!("pas de décision pour {input}");
            refused("Grove n'a pas répondu à la demande de permission.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_handshake_and_call() {
        let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}});
        let response = handle(&init, |_| unreachable!()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), |_| unreachable!()).is_none());

        let list = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}), |_| unreachable!()).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "permission");

        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                          "params": {"name": "permission", "arguments": {"tool_name": "Bash", "input": {"command": "ls"}}}});
        let response = handle(&call, |args| decision(true, &args["input"])).unwrap();
        let text: Value = serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, json!({"behavior": "allow", "updatedInput": {"command": "ls"}}));

        let unknown = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}), |_| unreachable!()).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn config_names_the_server_and_tool() {
        let config: Value = serde_json::from_str(&mcp_config(Path::new("/bin/grove"), Path::new("/run/g.sock"), "t1")).unwrap();
        let server = &config["mcpServers"]["grove"];
        assert_eq!((server["command"].as_str(), server["args"][0].as_str()), (Some("/bin/grove"), Some("mcp-permission")));
        assert_eq!(server["env"]["GROVE_TAB_ID"], "t1");
        assert_eq!(permission_tool(), "mcp__grove__permission");
    }
}
