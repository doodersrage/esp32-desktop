//! Minimal QMP client: newline-delimited JSON over a Unix socket.

use std::path::Path;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::Error;

pub struct Qmp {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    events: Vec<Value>,
}

impl Qmp {
    /// Connect, read the greeting and enter command mode.
    pub async fn connect(path: &Path) -> Result<Self, Error> {
        Self::handshake(UnixStream::connect(path).await?).await
    }

    pub async fn handshake(stream: UnixStream) -> Result<Self, Error> {
        let (r, w) = stream.into_split();
        let mut qmp = Qmp {
            reader: BufReader::new(r),
            writer: w,
            events: Vec::new(),
        };
        let greeting = qmp.read_message().await?;
        if greeting.get("QMP").is_none() {
            return Err(Error::QmpProtocol(format!(
                "unexpected greeting: {greeting}"
            )));
        }
        qmp.execute("qmp_capabilities", None).await?;
        Ok(qmp)
    }

    /// Run a command and return its `return` value. Events seen meanwhile are queued.
    pub async fn execute(
        &mut self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<Value, Error> {
        let mut msg = json!({ "execute": command });
        if let Some(a) = arguments {
            msg["arguments"] = a;
        }
        let mut line = serde_json::to_vec(&msg)?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;

        loop {
            let reply = self.read_message().await?;
            if let Some(ret) = reply.get("return") {
                return Ok(ret.clone());
            }
            if let Some(err) = reply.get("error") {
                let field = |k: &str| err.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                return Err(Error::QmpCommand {
                    command: command.to_owned(),
                    class: field("class"),
                    desc: field("desc"),
                });
            }
            if reply.get("event").is_some() {
                self.events.push(reply);
                continue;
            }
            return Err(Error::QmpProtocol(format!("unexpected message: {reply}")));
        }
    }

    /// Events received so far, oldest first. Draining clears the queue.
    pub fn drain_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    async fn read_message(&mut self) -> Result<Value, Error> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).await? == 0 {
            return Err(Error::QmpProtocol("connection closed".into()));
        }
        Ok(serde_json::from_str(&line)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A fake QMP server that checks each request and sends scripted replies.
    async fn fake_server(stream: UnixStream, script: Vec<(&'static str, Vec<&'static str>)>) {
        let (r, mut w) = stream.into_split();
        let mut r = BufReader::new(r);
        w.write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
            .await
            .unwrap();
        for (expect, replies) in script {
            let mut line = String::new();
            r.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(v["execute"], expect);
            for reply in replies {
                w.write_all(reply.as_bytes()).await.unwrap();
                w.write_all(b"\n").await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn negotiates_and_runs_commands() {
        let (a, b) = UnixStream::pair().unwrap();
        let server = tokio::spawn(fake_server(
            b,
            vec![
                ("qmp_capabilities", vec![r#"{"return": {}}"#]),
                (
                    "system_reset",
                    vec![
                        r#"{"event": "RESET", "data": {"guest": false}}"#,
                        r#"{"return": {}}"#,
                    ],
                ),
                (
                    "bogus",
                    vec![r#"{"error": {"class": "CommandNotFound", "desc": "nope"}}"#],
                ),
            ],
        ));
        let mut qmp = Qmp::handshake(a).await.unwrap();
        qmp.execute("system_reset", None).await.unwrap();
        let events = qmp.drain_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "RESET");

        match qmp.execute("bogus", None).await {
            Err(Error::QmpCommand { class, .. }) => assert_eq!(class, "CommandNotFound"),
            other => panic!("expected command error, got {other:?}", other = other.err()),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_non_qmp_peer() {
        let (a, mut b) = UnixStream::pair().unwrap();
        b.write_all(b"{\"hello\": 1}\n").await.unwrap();
        assert!(matches!(
            Qmp::handshake(a).await,
            Err(Error::QmpProtocol(_))
        ));
    }
}
