//! JSON-RPC 2.0 over a Unix socket, one JSON object per line. See `docs/rpc.md`.
//!
//! The server side is a [`Router`] of async method handlers served by [`serve`];
//! the client side is [`Client`], used by the CLI and tests.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Largest message accepted, in bytes. Longer lines close the connection.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Directory for the core's socket and lock, under `$XDG_RUNTIME_DIR`.
pub const RUNTIME_DIR: &str = "esp32bench";
pub const SOCKET_NAME: &str = "core.sock";

/// Default socket path: `$XDG_RUNTIME_DIR/esp32bench/core.sock`.
pub fn default_socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join(RUNTIME_DIR).join(SOCKET_NAME))
}

/// Error codes. Protocol codes are JSON-RPC's; application codes are listed in `docs/rpc.md`.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const NOT_FOUND: i64 = -32000;
    pub const BUSY: i64 = -32001;
    pub const CONSENT_REQUIRED: i64 = -32002;
    pub const UNAVAILABLE: i64 = -32003;
    pub const CANCELLED: i64 = -32004;
    pub const FAILED: i64 = -32005;
}

/// An error returned to the caller as a JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message} ({code})")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(code::INTERNAL, message)
    }

    fn to_json(&self) -> Value {
        let mut e = json!({ "code": self.code, "message": self.message });
        if let Some(d) = &self.data {
            e["data"] = d.clone();
        }
        e
    }
}

/// Deserialize a method's params, mapping failures to "Invalid params".
pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

/// Per-connection context handed to every handler.
#[derive(Clone)]
pub struct Ctx {
    outbox: mpsc::UnboundedSender<Value>,
    subs: Arc<Subscriptions>,
    connection: u64,
}

/// Source of connection ids, unique for the life of the process.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// The connection's live subscriptions: id to the task producing its events.
#[derive(Default)]
struct Subscriptions {
    next: AtomicU64,
    tasks: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
}

impl Subscriptions {
    fn abort_all(&self) {
        let tasks = match self.tasks.lock() {
            Ok(mut t) => std::mem::take(&mut *t),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        for (_, t) in tasks {
            t.abort();
        }
    }
}

/// Handle given to a subscription's task for sending its events.
pub struct Subscription {
    id: u64,
    outbox: mpsc::UnboundedSender<Value>,
}

impl Subscription {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Send an event. `params` must be an object; the subscription id is added to it.
    /// Returns false once the client is gone.
    pub fn send(&self, method: &str, mut params: Value) -> bool {
        if let Some(o) = params.as_object_mut() {
            o.insert("subscription".into(), json!(self.id));
        }
        self.outbox
            .send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .is_ok()
    }
}

impl Ctx {
    /// Send a notification to this connection's client.
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.outbox.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// This connection's id. Subscription ids are only unique within a connection, so
    /// state kept per subscription outside the connection is keyed by both.
    pub fn connection_id(&self) -> u64 {
        self.connection
    }

    /// True once the client has disconnected; long-running work should stop.
    pub fn is_closed(&self) -> bool {
        self.outbox.is_closed()
    }

    /// Start a subscription: `task` runs until it returns, the client unsubscribes, or
    /// the connection closes, whichever comes first. Returns the subscription id.
    pub fn subscribe<F, Fut>(&self, task: F) -> u64
    where
        F: FnOnce(Subscription) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let id = self.subs.next.fetch_add(1, Ordering::Relaxed) + 1;
        let sub = Subscription {
            id,
            outbox: self.outbox.clone(),
        };
        let handle = tokio::spawn(task(sub)).abort_handle();
        match self.subs.tasks.lock() {
            Ok(mut t) => {
                t.insert(id, handle);
            }
            // A poisoned map means a bug elsewhere; do not leave the task running.
            Err(_) => handle.abort(),
        }
        id
    }

    /// Stop a subscription. Returns false if this connection has no such id.
    pub fn unsubscribe(&self, id: u64) -> bool {
        let handle = match self.subs.tasks.lock() {
            Ok(mut t) => t.remove(&id),
            Err(_) => None,
        };
        match handle {
            Some(h) => {
                h.abort();
                true
            }
            None => false,
        }
    }
}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>>;
type Handler = Arc<dyn Fn(Value, Ctx) -> HandlerFuture + Send + Sync>;

/// Method name to handler.
#[derive(Default, Clone)]
pub struct Router {
    methods: HashMap<String, Handler>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Params arrive as a JSON object (`{}` when omitted).
    pub fn method<F, Fut>(mut self, name: &str, f: F) -> Self
    where
        F: Fn(Value, Ctx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, RpcError>> + Send + 'static,
    {
        self.methods
            .insert(name.to_owned(), Arc::new(move |p, c| Box::pin(f(p, c))));
        self
    }

    /// Registered method names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.methods.keys().cloned().collect();
        v.sort();
        v
    }

    /// Handle one parsed message. Returns the response, or None for a notification.
    async fn dispatch(&self, msg: Value, ctx: Ctx) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(error_response(
                Value::Null,
                &RpcError::new(code::INVALID_REQUEST, "expected a request object"),
            ));
        };
        let id = obj.get("id").cloned();
        let valid_id = matches!(id, None | Some(Value::Number(_)) | Some(Value::String(_)));
        let method = obj.get("method").and_then(Value::as_str);
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || method.is_none()
            || !valid_id
        {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                &RpcError::new(code::INVALID_REQUEST, "not a JSON-RPC 2.0 request"),
            ));
        }
        let method = method.unwrap_or_default();
        let params = match obj.get("params") {
            None => json!({}),
            Some(p @ Value::Object(_)) => p.clone(),
            Some(_) => {
                return id.map(|id| {
                    error_response(id, &RpcError::invalid_params("params must be an object"))
                });
            }
        };
        let result = match self.methods.get(method) {
            Some(h) => h(params, ctx).await,
            None => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("unknown method `{method}`"),
            )),
        };
        // A request without an id is a notification: no response, even on error.
        let id = id?;
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => error_response(id, &e),
        })
    }
}

fn error_response(id: Value, e: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": e.to_json() })
}

/// Read one line of at most [`MAX_MESSAGE`] bytes. `Ok(None)` at end of stream.
async fn read_line<R: AsyncRead + Unpin>(r: &mut BufReader<R>) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = r
        .take(MAX_MESSAGE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    // A final line without a newline before EOF is accepted; an over-long one is not.
    if buf.last() != Some(&b'\n') && buf.len() > MAX_MESSAGE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Ok(Some(buf))
}

/// Accept connections forever, serving each with `router`.
pub async fn serve(listener: UnixListener, router: Arc<Router>) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let router = router.clone();
        tokio::spawn(async move {
            let _ = serve_connection(stream, router).await;
        });
    }
}

/// Serve one connection until the client closes it.
pub async fn serve_connection(stream: UnixStream, router: Arc<Router>) -> std::io::Result<()> {
    let (r, mut w) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(mut line) = serde_json::to_vec(&msg) else {
                continue;
            };
            line.push(b'\n');
            if w.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut reader = BufReader::new(r);
    let subs = Arc::new(Subscriptions::default());
    let ctx = Ctx {
        outbox: tx.clone(),
        subs: subs.clone(),
        connection: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
    };
    let result = loop {
        let line = match read_line(&mut reader).await {
            Ok(Some(l)) => l,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let msg: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(error_response(
                    Value::Null,
                    &RpcError::new(code::PARSE_ERROR, e.to_string()),
                ));
                continue;
            }
        };
        if msg.is_array() {
            let _ = tx.send(error_response(
                Value::Null,
                &RpcError::new(code::INVALID_REQUEST, "batch requests are not supported"),
            ));
            continue;
        }
        let router = router.clone();
        let ctx = ctx.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Some(resp) = router.dispatch(msg, ctx).await {
                let _ = tx.send(resp);
            }
        });
    };
    // Stop this connection's subscriptions: nobody is listening any more, and their
    // senders would otherwise keep the writer alive.
    subs.abort_all();
    // Dropping our senders lets the writer finish once in-flight handlers are done.
    drop(ctx);
    drop(tx);
    let _ = writer.await;
    result
}

/// Errors seen by a [`Client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("cannot connect to the core at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("bad message from the core: {0}")]
    Protocol(String),
    #[error("{0}")]
    Rpc(RpcError),
}

/// A simple sequential client: one request at a time, notifications are collected.
pub struct Client {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
    notifications: Vec<Value>,
}

impl Client {
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let s = UnixStream::connect(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.to_owned(),
                source,
            })?;
        let (r, w) = s.into_split();
        Ok(Client {
            reader: BufReader::new(r),
            writer: w,
            next_id: 1,
            notifications: Vec::new(),
        })
    }

    /// Call `method` and wait for its response.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line =
            serde_json::to_vec(&req).map_err(|e| ClientError::Protocol(e.to_string()))?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;

        loop {
            let Some(line) = read_line(&mut self.reader).await? else {
                return Err(ClientError::Protocol("connection closed".into()));
            };
            let msg: Value =
                serde_json::from_slice(&line).map_err(|e| ClientError::Protocol(e.to_string()))?;
            if msg.get("id").is_none() && msg.get("method").is_some() {
                self.notifications.push(msg);
                continue;
            }
            if msg.get("id") != Some(&json!(id)) {
                return Err(ClientError::Protocol(format!("unexpected message: {msg}")));
            }
            if let Some(r) = msg.get("result") {
                return Ok(r.clone());
            }
            let e = msg
                .get("error")
                .ok_or_else(|| ClientError::Protocol(format!("no result or error: {msg}")))?;
            return Err(ClientError::Rpc(RpcError {
                code: e
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or(code::INTERNAL),
                message: e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                data: e.get("data").cloned(),
            }));
        }
    }

    /// Wait for the next notification (one already received first).
    pub async fn next_notification(&mut self) -> Result<Value, ClientError> {
        if !self.notifications.is_empty() {
            return Ok(self.notifications.remove(0));
        }
        loop {
            let Some(line) = read_line(&mut self.reader).await? else {
                return Err(ClientError::Protocol("connection closed".into()));
            };
            let msg: Value =
                serde_json::from_slice(&line).map_err(|e| ClientError::Protocol(e.to_string()))?;
            if msg.get("id").is_none() && msg.get("method").is_some() {
                return Ok(msg);
            }
        }
    }

    /// Notifications received so far, oldest first. Draining clears them.
    pub fn drain_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_router() -> Router {
        Router::new()
            .method("test.echo", |p, _| async move { Ok(p) })
            .method("test.fail", |_, _| async move {
                Err(RpcError::new(code::NOT_FOUND, "no such thing").with_data(json!({"what": "x"})))
            })
            .method("test.slow", |_, _| async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(json!("slow"))
            })
            .method("test.notify", |_, ctx: Ctx| async move {
                ctx.notify("test.event", json!({ "subscription": 1, "n": 7 }));
                Ok(json!(null))
            })
    }

    /// Decrements a counter when dropped, to observe when a subscription task ends.
    struct Live(Arc<std::sync::atomic::AtomicUsize>);
    impl Live {
        fn new(c: &Arc<std::sync::atomic::AtomicUsize>) -> Self {
            c.fetch_add(1, Ordering::SeqCst);
            Live(c.clone())
        }
    }
    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A router whose `test.ticks` starts a subscription sending a tick every 20 ms.
    fn ticks_router(live: Arc<std::sync::atomic::AtomicUsize>) -> Router {
        Router::new()
            .method("test.ticks", move |_, ctx: Ctx| {
                let live = live.clone();
                async move {
                    let id = ctx.subscribe(|sub| async move {
                        let _live = Live::new(&live);
                        for n in 0u64.. {
                            if !sub.send("test.tick", json!({ "n": n })) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    });
                    Ok(json!({ "subscription": id }))
                }
            })
            .method("test.unsubscribe", |p, ctx: Ctx| async move {
                Ok(json!(
                    ctx.unsubscribe(p["subscription"].as_u64().unwrap_or(0))
                ))
            })
    }

    async fn serve_router(router: Router) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(serve(listener, Arc::new(router)));
        (dir, path)
    }

    #[tokio::test]
    async fn subscriptions_stream_until_unsubscribed() {
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_d, path) = serve_router(ticks_router(live.clone())).await;
        let mut c = Client::connect(&path).await.unwrap();
        let id = c.call("test.ticks", json!({})).await.unwrap()["subscription"]
            .as_u64()
            .unwrap();
        for _ in 0..3 {
            let n = tokio::time::timeout(Duration::from_secs(2), c.next_notification())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(n["method"], "test.tick");
            assert_eq!(n["params"]["subscription"], id);
        }
        assert_eq!(
            c.call("test.unsubscribe", json!({ "subscription": id }))
                .await
                .unwrap(),
            true
        );
        // Unknown or already-stopped ids are refused.
        assert_eq!(
            c.call("test.unsubscribe", json!({ "subscription": id }))
                .await
                .unwrap(),
            false
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            live.load(Ordering::SeqCst),
            0,
            "task still running after unsubscribe"
        );
    }

    #[tokio::test]
    async fn subscriptions_stop_when_the_client_disconnects() {
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_d, path) = serve_router(ticks_router(live.clone())).await;
        let mut c = Client::connect(&path).await.unwrap();
        c.call("test.ticks", json!({})).await.unwrap();
        c.call("test.ticks", json!({})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(live.load(Ordering::SeqCst), 2);
        drop(c);
        for _ in 0..100 {
            if live.load(Ordering::SeqCst) == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("subscription tasks still running after the client left");
    }

    /// A server on a real Unix socket in a temp dir, and the socket path.
    async fn server() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(serve(listener, Arc::new(test_router())));
        (dir, path)
    }

    async fn raw(path: &Path, lines: &[&str]) -> Vec<Value> {
        let s = UnixStream::connect(path).await.unwrap();
        let (r, mut w) = s.into_split();
        for l in lines {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        let mut reader = BufReader::new(r);
        let mut out = Vec::new();
        for _ in 0..lines.len() {
            let line = tokio::time::timeout(Duration::from_secs(2), read_line(&mut reader))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            out.push(serde_json::from_slice(&line).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn round_trips_success_and_error() {
        let (_d, path) = server().await;
        let mut c = Client::connect(&path).await.unwrap();
        assert_eq!(
            c.call("test.echo", json!({"a": 1})).await.unwrap(),
            json!({"a": 1})
        );
        match c.call("test.fail", json!({})).await {
            Err(ClientError::Rpc(e)) => {
                assert_eq!(e.code, code::NOT_FOUND);
                assert_eq!(e.data, Some(json!({"what": "x"})));
            }
            other => panic!("expected an RPC error, got {other:?}"),
        }
        match c.call("nope.nope", json!({})).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::METHOD_NOT_FOUND),
            other => panic!("expected method not found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_malformed_messages() {
        let (_d, path) = server().await;
        let out = raw(
            &path,
            &[
                "{not json",
                r#"[{"jsonrpc":"2.0","id":1,"method":"test.echo"}]"#,
                r#"{"jsonrpc":"1.0","id":2,"method":"test.echo"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"test.echo","params":[1,2]}"#,
            ],
        )
        .await;
        let codes: Vec<i64> = out
            .iter()
            .map(|m| m["error"]["code"].as_i64().unwrap())
            .collect();
        assert_eq!(
            codes,
            [
                code::PARSE_ERROR,
                code::INVALID_REQUEST,
                code::INVALID_REQUEST,
                code::INVALID_PARAMS
            ]
        );
    }

    #[tokio::test]
    async fn handles_requests_concurrently() {
        let (_d, path) = server().await;
        // The slow request is sent first, but the fast one answers first.
        let out = raw(
            &path,
            &[
                r#"{"jsonrpc":"2.0","id":"slow","method":"test.slow"}"#,
                r#"{"jsonrpc":"2.0","id":"fast","method":"test.echo"}"#,
            ],
        )
        .await;
        assert_eq!(out[0]["id"], "fast");
        assert_eq!(out[1]["id"], "slow");
    }

    #[tokio::test]
    async fn delivers_notifications_and_skips_notification_responses() {
        let (_d, path) = server().await;
        let mut c = Client::connect(&path).await.unwrap();
        c.call("test.notify", json!({})).await.unwrap();
        let n = c.drain_notifications();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["method"], "test.event");
        assert_eq!(n[0]["params"]["n"], 7);

        // A request without an id gets no response; the next call still works.
        let s = UnixStream::connect(&path).await.unwrap();
        let (r, mut w) = s.into_split();
        w.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"test.echo\"}\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"test.echo\",\"params\":{\"x\":1}}\n")
            .await
            .unwrap();
        let mut reader = BufReader::new(r);
        let line = read_line(&mut reader).await.unwrap().unwrap();
        let v: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(v["id"], 9);
    }

    #[tokio::test]
    async fn closes_connection_on_oversized_message() {
        let (_d, path) = server().await;
        let s = UnixStream::connect(&path).await.unwrap();
        let (r, mut w) = s.into_split();
        let big = vec![b'x'; MAX_MESSAGE + 10];
        let _ = w.write_all(&big).await;
        let mut reader = BufReader::new(r);
        let got = tokio::time::timeout(Duration::from_secs(5), read_line(&mut reader))
            .await
            .unwrap();
        assert!(matches!(got, Ok(None) | Err(_)), "connection should close");
    }
}
