use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::protocol::{Message, MessageKind};

/// Réponse à une demande de permission, sur la connexion qui l'a apportée.
/// Détruite sans réponse, elle ferme la connexion : la demande est refusée.
pub struct Reply(Option<UnixStream>);

impl Reply {
    pub fn send(mut self, value: &serde_json::Value) {
        if let Some(mut stream) = self.0.take() {
            let _ = stream.write_all(format!("{value}\n").as_bytes());
        }
    }
}

/// Serveur du socket : reçoit les messages de `grove hook` / `grove statusline`.
/// Le socket est supprimé quand la valeur est détruite.
pub struct EventServer {
    path: PathBuf,
}

impl EventServer {
    /// `on_message` reçoit chaque message, et la réponse attendue pour une demande de permission.
    pub fn start(path: &Path, on_message: impl Fn(Message, Option<Reply>) + Send + 'static) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).with_context(|| format!("écoute sur {}", path.display()))?;

        std::thread::Builder::new().name("grove-claude-events".into()).spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let mut reader = BufReader::new(stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                match serde_json::from_str::<Message>(&line) {
                    Ok(message) if message.kind == MessageKind::Permission => {
                        let reply = Reply(Some(reader.into_inner()));
                        on_message(message, Some(reply));
                    }
                    Ok(message) => on_message(message, None),
                    Err(err) => log::debug!("message de hook illisible : {err}"),
                }
            }
        })?;
        Ok(Self { path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EventServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;

    use super::*;
    use crate::protocol::MessageKind;

    #[test]
    fn receives_messages_and_removes_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grove.sock");
        let (tx, rx) = mpsc::channel();
        let server = EventServer::start(&path, move |m, _| {
            let _ = tx.send(m);
        })
        .unwrap();

        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .write_all(br#"{"tab":"t1","kind":"hook","payload":{"hook_event_name":"Stop"}}"#)
            .unwrap();
        stream.write_all(b"\n").unwrap();
        drop(stream);

        let message = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((message.tab.as_str(), message.kind), ("t1", MessageKind::Hook));
        assert_eq!(message.payload["hook_event_name"], "Stop");

        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn permission_requests_get_a_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grove.sock");
        let _server = EventServer::start(&path, |m, reply| {
            assert_eq!(m.kind, MessageKind::Permission);
            reply.expect("réponse attendue").send(&serde_json::json!({"behavior": "allow"}));
        })
        .unwrap();

        let mut stream = UnixStream::connect(&path).unwrap();
        stream.write_all(br#"{"tab":"t1","kind":"permission","payload":{"tool_name":"Bash"}}"#).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        assert_eq!(reply.trim(), r#"{"behavior":"allow"}"#);
    }
}
