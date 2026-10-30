//! The stdio JSON-RPC client: one writer thread, one reader thread, one stderr thread and
//! one process watcher. Results are delivered through flume channels.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};

use crate::convert::{self, ProgressTracker};
use crate::framing::{FrameReader, encode};
use crate::uri::path_to_uri;
use crate::{
    CompletionItem, Location, LspEvent, Position, SemanticToken, ServerConfig, TextChange,
};

/// JSON-RPC `MethodNotFound`.
const METHOD_NOT_FOUND: i64 = -32601;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// An error response from the server.
#[derive(Debug)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "language server error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for ResponseError {}

type Callback = Box<dyn FnOnce(Result<Value>, &Shared) + Send>;

#[derive(Default)]
struct State {
    next_id: i64,
    pending: HashMap<i64, Callback>,
    /// Initialize handshake done; messages go straight to the writer.
    ready: bool,
    /// Process is gone; everything fails fast.
    exited: bool,
    /// `shutdown()` was called; new requests are rejected.
    closing: bool,
    /// Messages issued before `ready`.
    queue: Vec<Value>,
    /// `None` until the server announced its capabilities.
    semantic_tokens_supported: Option<bool>,
    legend: Vec<String>,
    capabilities: Value,
    /// Server uses pull diagnostics (`textDocument/diagnostic`) instead of publishing them.
    pull_diagnostics: bool,
    /// Open documents by URI: (path, latest version).
    open_docs: HashMap<String, (PathBuf, i32)>,
    progress: ProgressTracker,
}

struct Shared {
    state: Mutex<State>,
    exit_cv: Condvar,
    writer: flume::Sender<Value>,
    events: flume::Sender<LspEvent>,
    child: Mutex<Child>,
    exit_reported: AtomicBool,
    shutdown_started: AtomicBool,
    language_id: String,
    workspace_folder: Value,
    /// URIs whose diagnostics should be pulled (debounced by the puller thread).
    pull_tx: flume::Sender<String>,
}

/// Owns the client; dropping the last clone shuts the server down.
struct Handle(Arc<Shared>);

impl Drop for Handle {
    fn drop(&mut self) {
        Shared::shutdown(&self.0);
    }
}

/// A handle to a running language server. Cheap to clone; `Send + Sync`.
#[derive(Clone)]
pub struct LspClient {
    handle: Arc<Handle>,
}

impl LspClient {
    /// Spawns the server and performs `initialize`/`initialized` in the background; returns
    /// immediately. Requests issued before [`LspEvent::Ready`] are queued and sent afterwards.
    pub fn start(config: ServerConfig) -> Result<(LspClient, flume::Receiver<LspEvent>)> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&config.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start language server `{}`", config.command))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        let (writer_tx, writer_rx) = flume::unbounded();
        let (event_tx, event_rx) = flume::unbounded();
        let (pull_tx, pull_rx) = flume::unbounded();
        let root_uri = path_to_uri(&config.root);
        let root_name = config
            .root
            .file_name()
            .map_or_else(|| root_uri.clone(), |n| n.to_string_lossy().into_owned());
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next_id: 1,
                ..State::default()
            }),
            exit_cv: Condvar::new(),
            writer: writer_tx,
            events: event_tx.clone(),
            child: Mutex::new(child),
            exit_reported: AtomicBool::new(false),
            shutdown_started: AtomicBool::new(false),
            language_id: config.language_id.clone(),
            workspace_folder: json!({ "uri": root_uri, "name": root_name }),
            pull_tx,
        });

        spawn("bf-lsp-writer", move || writer_loop(stdin, writer_rx))?;
        let reader = shared.clone();
        spawn("bf-lsp-reader", move || reader.reader_loop(stdout))?;
        spawn("bf-lsp-stderr", move || stderr_loop(stderr, event_tx))?;
        let watcher = shared.clone();
        spawn("bf-lsp-watch", move || watcher.watch_process())?;
        let puller = Arc::downgrade(&shared);
        spawn("bf-lsp-diagnostics", move || {
            diagnostics_puller(puller, pull_rx)
        })?;

        let params = initialize_params(&config, &shared.workspace_folder);
        shared.request_now(
            "initialize",
            params,
            Box::new(|result, shared| shared.on_initialize(result)),
        );
        Ok((
            LspClient {
                handle: Arc::new(Handle(shared)),
            },
            event_rx,
        ))
    }

    fn shared(&self) -> &Shared {
        &self.handle.0
    }

    /// `textDocument/didOpen`.
    pub fn did_open(&self, path: &Path, version: i32, text: &str) {
        let shared = self.shared();
        let uri = path_to_uri(path);
        shared
            .state
            .lock()
            .open_docs
            .insert(uri.clone(), (path.to_path_buf(), version));
        let doc = json!({ "uri": uri, "languageId": shared.language_id, "version": version, "text": text });
        shared.notify("textDocument/didOpen", json!({ "textDocument": doc }));
        shared.schedule_pull(uri);
    }

    /// `textDocument/didChange` (incremental or full changes, forwarded as given).
    pub fn did_change(&self, path: &Path, version: i32, changes: Vec<TextChange>) {
        let changes: Vec<Value> = changes.iter().map(convert::change_json).collect();
        let shared = self.shared();
        let uri = path_to_uri(path);
        if let Some(doc) = shared.state.lock().open_docs.get_mut(&uri) {
            doc.1 = version;
        }
        let doc = json!({ "uri": uri, "version": version });
        shared.notify(
            "textDocument/didChange",
            json!({ "textDocument": doc, "contentChanges": changes }),
        );
        shared.schedule_pull(uri);
    }

    /// `textDocument/didSave`, optionally including the saved text.
    pub fn did_save(&self, path: &Path, text: Option<&str>) {
        let uri = path_to_uri(path);
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        self.shared().notify("textDocument/didSave", params);
        self.shared().schedule_pull(uri);
    }

    /// `textDocument/didClose`.
    pub fn did_close(&self, path: &Path) {
        let uri = path_to_uri(path);
        self.shared().state.lock().open_docs.remove(&uri);
        self.shared().notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
    }

    /// `textDocument/completion` at `position`.
    pub fn completion(
        &self,
        path: &Path,
        position: Position,
    ) -> flume::Receiver<Result<Vec<CompletionItem>>> {
        let mut params = position_params(path, position);
        params["context"] = json!({ "triggerKind": 1 });
        self.typed_request("textDocument/completion", params, |v, _| {
            Ok(convert::parse_completion(&v))
        })
    }

    /// `textDocument/definition` at `position`.
    pub fn definition(
        &self,
        path: &Path,
        position: Position,
    ) -> flume::Receiver<Result<Vec<Location>>> {
        self.typed_request(
            "textDocument/definition",
            position_params(path, position),
            |v, _| Ok(convert::parse_locations(&v)),
        )
    }

    /// `textDocument/hover`, flattened to markdown/plain text.
    pub fn hover(
        &self,
        path: &Path,
        position: Position,
    ) -> flume::Receiver<Result<Option<String>>> {
        self.typed_request(
            "textDocument/hover",
            position_params(path, position),
            |v, _| Ok(convert::flatten_hover(&v)),
        )
    }

    /// `textDocument/semanticTokens/full`; `Ok(empty)` if the server does not support it.
    pub fn semantic_tokens(&self, path: &Path) -> flume::Receiver<Result<Vec<SemanticToken>>> {
        if self.shared().state.lock().semantic_tokens_supported == Some(false) {
            let (tx, rx) = flume::bounded(1);
            let _ = tx.send(Ok(Vec::new()));
            return rx;
        }
        let (tx, rx) = flume::bounded(1);
        let params = json!({ "textDocument": { "uri": path_to_uri(path) } });
        let callback: Callback = Box::new(move |result, shared| {
            let result = match result {
                Ok(v) => {
                    let legend = shared.state.lock().legend.clone();
                    Ok(convert::decode_semantic_tokens(
                        &convert::semantic_data(&v),
                        &legend,
                    ))
                }
                Err(e)
                    if e.downcast_ref::<ResponseError>()
                        .is_some_and(|e| e.code == METHOD_NOT_FOUND) =>
                {
                    Ok(Vec::new())
                }
                Err(e) => Err(e),
            };
            let _ = tx.send(result);
        });
        self.shared()
            .request("textDocument/semanticTokens/full", params, callback);
        rx
    }

    /// The semantic token type names advertised by the server (empty until ready/unsupported).
    pub fn semantic_token_legend(&self) -> Vec<String> {
        self.shared().state.lock().legend.clone()
    }

    /// The raw `ServerCapabilities` from the initialize result (`null` until ready).
    pub fn server_capabilities(&self) -> Value {
        self.shared().state.lock().capabilities.clone()
    }

    /// Whether the initialize handshake has completed and the server is still running.
    pub fn is_ready(&self) -> bool {
        let state = self.shared().state.lock();
        state.ready && !state.exited
    }

    /// Sends `shutdown` + `exit` in the background and kills the process if it is still alive
    /// after 2 s. Also runs when the last clone is dropped. Non-blocking.
    pub fn shutdown(&self) {
        Shared::shutdown(&self.handle.0);
    }

    /// Blocks until the server process has exited or `timeout` elapsed; returns `true` if exited.
    pub fn wait_for_exit(&self, timeout: Duration) -> bool {
        let shared = self.shared();
        let deadline = Instant::now() + timeout;
        let mut state = shared.state.lock();
        while !state.exited {
            if shared.exit_cv.wait_until(&mut state, deadline).timed_out() {
                break;
            }
        }
        state.exited
    }

    fn typed_request<T: Send + 'static>(
        &self,
        method: &str,
        params: Value,
        convert: impl FnOnce(Value, &Shared) -> Result<T> + Send + 'static,
    ) -> flume::Receiver<Result<T>> {
        let (tx, rx) = flume::bounded(1);
        let callback: Callback = Box::new(move |result, shared| {
            let _ = tx.send(result.and_then(|v| convert(v, shared)));
        });
        self.shared().request(method, params, callback);
        rx
    }
}

impl Shared {
    fn emit(&self, event: LspEvent) {
        let _ = self.events.send(event);
    }

    /// Sends a request, queueing it until the handshake is done.
    fn request(&self, method: &str, params: Value, callback: Callback) {
        self.request_inner(method, params, callback, false);
    }

    /// Sends a request immediately, bypassing the pre-initialize queue and the closing check.
    fn request_now(&self, method: &str, params: Value, callback: Callback) {
        self.request_inner(method, params, callback, true);
    }

    fn request_inner(&self, method: &str, params: Value, callback: Callback, force: bool) {
        let mut state = self.state.lock();
        if state.exited || (state.closing && !force) {
            drop(state);
            callback(Err(anyhow!("language server is not running")), self);
            return;
        }
        let id = state.next_id;
        state.next_id += 1;
        state.pending.insert(id, callback);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if state.ready || force {
            let _ = self.writer.send(message);
        } else {
            state.queue.push(message);
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let mut state = self.state.lock();
        if state.exited || state.closing {
            return;
        }
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if state.ready {
            let _ = self.writer.send(message);
        } else {
            state.queue.push(message);
        }
    }

    fn send_now(&self, message: Value) {
        let _ = self.writer.send(message);
    }

    fn on_initialize(&self, result: Result<Value>) {
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                self.emit(LspEvent::Exited(format!("initialize failed: {e:#}")));
                self.kill();
                return;
            }
        };
        let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        log::debug!("server capabilities: {capabilities}");
        let provider = capabilities
            .get("semanticTokensProvider")
            .filter(|p| p.is_object());
        let legend = provider.and_then(convert::parse_legend);
        {
            let mut state = self.state.lock();
            if state.exited {
                return;
            }
            state.semantic_tokens_supported = Some(legend.is_some());
            state.pull_diagnostics = capabilities
                .get("diagnosticProvider")
                .is_some_and(|p| p.is_object() || p == true);
            state.capabilities = capabilities;
            state.legend = legend.unwrap_or_default();
            self.send_now(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
            for message in state.queue.drain(..) {
                let _ = self.writer.send(message);
            }
            state.ready = true;
        }
        self.emit(LspEvent::Ready);
        self.schedule_pull_all();
    }

    fn reader_loop(self: Arc<Self>, mut stdout: ChildStdout) {
        let mut frames = FrameReader::default();
        let mut buf = vec![0u8; 64 * 1024];
        let reason = loop {
            match stdout.read(&mut buf) {
                Ok(0) => break "language server closed its output".to_string(),
                Ok(n) => frames.push(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => break format!("failed to read from language server: {e}"),
            }
            loop {
                match frames.next_frame() {
                    Ok(Some(body)) => self.handle_frame(&body),
                    Ok(None) => break,
                    Err(e) => log::warn!("bf-lsp: {e}"),
                }
            }
        };
        // Give the process watcher a moment to report the real exit status.
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut state = self.state.lock();
        while !state.exited && !self.exit_cv.wait_until(&mut state, deadline).timed_out() {}
        drop(state);
        self.on_exit(reason);
    }

    fn watch_process(self: Arc<Self>) {
        loop {
            let status = self.child.lock().try_wait();
            match status {
                Ok(Some(status)) => {
                    return self.on_exit(format!("language server exited ({status})"));
                }
                Ok(None) => {}
                Err(e) => {
                    return self.on_exit(format!("failed to query language server process: {e}"));
                }
            }
            if self.state.lock().exited {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// Marks the server as gone, fails pending requests and emits `Exited` (once).
    fn on_exit(&self, reason: String) {
        if self.exit_reported.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending = {
            let mut state = self.state.lock();
            state.exited = true;
            state.ready = false;
            state.queue.clear();
            std::mem::take(&mut state.pending)
        };
        self.exit_cv.notify_all();
        for (_, callback) in pending {
            callback(Err(anyhow!("{reason}")), self);
        }
        log::info!("bf-lsp: {reason}");
        self.emit(LspEvent::Exited(reason));
    }

    fn handle_frame(&self, body: &[u8]) {
        let message: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "bf-lsp: invalid JSON from server ({e}): {}",
                    String::from_utf8_lossy(body)
                );
                return;
            }
        };
        log::debug!("<- {message}");
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").filter(|id| !id.is_null());
        let params = message.get("params").unwrap_or(&Value::Null);
        match (method, id) {
            (Some(method), Some(id)) => self.handle_server_request(method, id.clone(), params),
            (Some(method), None) => self.handle_notification(method, params),
            (None, Some(id)) => self.handle_response(id, &message),
            (None, None) => log::warn!("bf-lsp: unexpected message: {message}"),
        }
    }

    fn handle_response(&self, id: &Value, message: &Value) {
        let Some(id) = id
            .as_i64()
            .or_else(|| id.as_str().and_then(|s| s.parse().ok()))
        else {
            log::warn!("bf-lsp: response with unknown id {id}");
            return;
        };
        let Some(callback) = self.state.lock().pending.remove(&id) else {
            log::debug!("bf-lsp: response for unknown request {id}");
            return;
        };
        let result = match message.get("error") {
            Some(error) if !error.is_null() => Err(anyhow::Error::new(ResponseError {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })),
            _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        callback(result, self);
    }

    fn handle_notification(&self, method: &str, params: &Value) {
        match method {
            "textDocument/publishDiagnostics" => match convert::parse_publish_diagnostics(params) {
                Some((path, version, diagnostics)) => self.emit(LspEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                }),
                None => log::warn!("bf-lsp: malformed publishDiagnostics: {params}"),
            },
            "$/progress" => {
                let text = self.state.lock().progress.handle(params);
                if let Some(text) = text {
                    self.emit(LspEvent::Progress(text));
                }
                // Diagnostics computed while the project was still loading are incomplete.
                if params.pointer("/value/kind").and_then(Value::as_str) == Some("end") {
                    self.schedule_pull_all();
                }
            }
            "window/logMessage" | "window/showMessage" | "$/logTrace" => {
                if let Some(message) = params.get("message").and_then(Value::as_str) {
                    self.emit(LspEvent::Log(message.to_owned()));
                }
            }
            _ => log::debug!("bf-lsp: ignoring notification {method}"),
        }
    }

    /// Answers server -> client requests so the server never blocks waiting on us.
    fn handle_server_request(&self, method: &str, id: Value, params: &Value) {
        let result = match method {
            "workspace/configuration" => {
                // An empty object per requested section: csharp-ls throws on `null`.
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                items.iter().map(|_| json!({})).collect()
            }
            "workspace/workspaceFolders" => json!([self.workspace_folder]),
            "client/registerCapability" => {
                self.on_register_capability(params);
                Value::Null
            }
            "window/showMessageRequest" => {
                if let Some(message) = params.get("message").and_then(Value::as_str) {
                    self.emit(LspEvent::Log(message.to_owned()));
                }
                Value::Null
            }
            "workspace/applyEdit" => json!({ "applied": false }),
            "window/showDocument" => json!({ "success": false }),
            "workspace/diagnostic/refresh" => {
                self.schedule_pull_all();
                Value::Null
            }
            "window/workDoneProgress/create"
            | "client/unregisterCapability"
            | "workspace/semanticTokens/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/inlineValue/refresh"
            | "workspace/foldingRange/refresh" => Value::Null,
            _ => {
                log::debug!("bf-lsp: unsupported server request {method}");
                return self.send_now(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": METHOD_NOT_FOUND, "message": format!("unsupported: {method}") }
                }));
            }
        };
        self.send_now(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// Picks up a dynamically registered semantic tokens provider (and its legend).
    fn on_register_capability(&self, params: &Value) {
        let registrations = params.get("registrations").and_then(Value::as_array);
        for registration in registrations.into_iter().flatten() {
            if registration.get("method").and_then(Value::as_str)
                != Some("textDocument/semanticTokens")
            {
                continue;
            }
            if let Some(legend) = registration
                .get("registerOptions")
                .and_then(convert::parse_legend)
            {
                let mut state = self.state.lock();
                state.semantic_tokens_supported = Some(true);
                state.legend = legend;
            }
        }
    }

    /// Queues a diagnostics pull for `uri` if the server uses pull diagnostics.
    /// Before the handshake this is a no-op; `on_initialize` pulls all open documents.
    fn schedule_pull(&self, uri: String) {
        let state = self.state.lock();
        if state.ready && state.pull_diagnostics {
            let _ = self.pull_tx.send(uri);
        }
    }

    fn schedule_pull_all(&self) {
        let uris: Vec<String> = self.state.lock().open_docs.keys().cloned().collect();
        for uri in uris {
            self.schedule_pull(uri);
        }
    }

    /// Sends `textDocument/diagnostic` and reports the result as [`LspEvent::Diagnostics`].
    fn pull_diagnostics(&self, uri: String) {
        let Some((path, version)) = self.state.lock().open_docs.get(&uri).cloned() else {
            return;
        };
        let params = json!({ "textDocument": { "uri": uri } });
        self.request(
            "textDocument/diagnostic",
            params,
            Box::new(move |result, shared| match result {
                Ok(report) if report.get("kind").and_then(Value::as_str) == Some("full") => {
                    let diagnostics =
                        convert::parse_diagnostic_list(report.get("items").unwrap_or(&Value::Null));
                    shared.emit(LspEvent::Diagnostics {
                        path,
                        version: Some(version),
                        diagnostics,
                    });
                }
                Ok(_) => {}
                Err(e) => log::debug!("bf-lsp: diagnostic pull for {uri} failed: {e:#}"),
            }),
        );
    }

    fn kill(&self) {
        if let Err(e) = self.child.lock().kill() {
            log::debug!("bf-lsp: kill failed: {e}");
        }
    }

    fn shutdown(this: &Arc<Self>) {
        if this.shutdown_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let ready = {
            let mut state = this.state.lock();
            if state.exited {
                return;
            }
            state.closing = true;
            state.ready
        };
        if !ready {
            // Cannot shut down gracefully before the handshake completed.
            this.kill();
            return;
        }
        let (done_tx, done_rx) = flume::bounded(1);
        this.request_now(
            "shutdown",
            Value::Null,
            Box::new(move |_, _| {
                let _ = done_tx.send(());
            }),
        );
        let shared = this.clone();
        let spawned = spawn("bf-lsp-shutdown", move || {
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            if done_rx.recv_deadline(deadline).is_ok() {
                shared.send_now(json!({ "jsonrpc": "2.0", "method": "exit" }));
            }
            let mut state = shared.state.lock();
            while !state.exited && !shared.exit_cv.wait_until(&mut state, deadline).timed_out() {}
            let exited = state.exited;
            drop(state);
            if !exited {
                log::info!("bf-lsp: server did not exit in time; killing it");
                shared.kill();
            }
        });
        if spawned.is_err() {
            this.kill();
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .map(drop)
        .with_context(|| format!("failed to spawn thread {name}"))
}

/// Debounces diagnostic pull requests: waits until no new URI arrived for a short while
/// (at most ~1 s after the first) and then pulls each distinct URI once.
fn diagnostics_puller(shared: std::sync::Weak<Shared>, uris: flume::Receiver<String>) {
    const QUIET: Duration = Duration::from_millis(250);
    const MAX_DELAY: Duration = Duration::from_secs(1);
    while let Ok(first) = uris.recv() {
        let started = Instant::now();
        let mut batch = HashSet::from([first]);
        while started.elapsed() < MAX_DELAY {
            match uris.recv_timeout(QUIET) {
                Ok(uri) => {
                    batch.insert(uri);
                }
                Err(_) => break,
            }
        }
        let Some(shared) = shared.upgrade() else {
            return;
        };
        for uri in batch {
            shared.pull_diagnostics(uri);
        }
    }
}

fn writer_loop(mut stdin: ChildStdin, messages: flume::Receiver<Value>) {
    for message in messages.iter() {
        log::debug!("-> {message}");
        let body = match serde_json::to_vec(&message) {
            Ok(body) => body,
            Err(e) => {
                log::error!("bf-lsp: failed to serialize message: {e}");
                continue;
            }
        };
        if let Err(e) = stdin.write_all(&encode(&body)).and_then(|()| stdin.flush()) {
            log::debug!("bf-lsp: writing to server failed: {e}");
            break;
        }
    }
}

fn stderr_loop(stderr: impl Read, events: flume::Sender<LspEvent>) {
    for line in BufReader::new(stderr).lines() {
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => {
                let _ = events.send(LspEvent::Log(line));
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {}
            Err(_) => break,
        }
    }
}

fn position_params(path: &Path, position: Position) -> Value {
    json!({ "textDocument": { "uri": path_to_uri(path) }, "position": convert::position_json(position) })
}

const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
    "label",
];

const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

fn initialize_params(config: &ServerConfig, workspace_folder: &Value) -> Value {
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "ByteForge", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": workspace_folder["uri"],
        "rootPath": config.root.to_string_lossy(),
        "workspaceFolders": [workspace_folder],
        "trace": "off",
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "window": {
                "workDoneProgress": true,
                "showMessage": { "messageActionItem": { "additionalPropertiesSupport": false } }
            },
            "workspace": {
                "workspaceFolders": true,
                "configuration": true,
                "applyEdit": false,
                "diagnostics": { "refreshSupport": true },
                "didChangeConfiguration": { "dynamicRegistration": false }
            },
            "textDocument": {
                "synchronization": { "dynamicRegistration": false, "didSave": true, "willSave": false, "willSaveWaitUntil": false },
                "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
                "completion": {
                    "dynamicRegistration": false,
                    "contextSupport": true,
                    "completionItem": {
                        "snippetSupport": false,
                        "insertReplaceSupport": false,
                        "documentationFormat": ["markdown", "plaintext"]
                    }
                },
                "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
                "diagnostic": { "dynamicRegistration": false, "relatedDocumentSupport": false },
                "definition": { "dynamicRegistration": false, "linkSupport": false },
                "semanticTokens": {
                    "dynamicRegistration": false,
                    "requests": { "full": true, "range": false },
                    "tokenTypes": TOKEN_TYPES,
                    "tokenModifiers": TOKEN_MODIFIERS,
                    "formats": ["relative"],
                    "overlappingTokenSupport": false,
                    "multilineTokenSupport": false
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync + Clone>() {}

    #[test]
    fn client_is_send_sync_clone() {
        assert_send_sync::<LspClient>();
    }

    #[test]
    fn start_failure_is_an_error() {
        let config = ServerConfig {
            command: "bf-lsp-definitely-not-a-real-binary".into(),
            args: vec![],
            root: std::env::temp_dir(),
            language_id: "csharp".into(),
        };
        assert!(LspClient::start(config).is_err());
    }
}
