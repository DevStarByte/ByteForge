//! A debug adapter for Blazor WebAssembly apps. It speaks DAP on stdin/stdout, like netcoredbg, so the
//! IDE's debugger drives it the same way; underneath it runs the app (`dotnet run`), opens it in a
//! Chromium-based browser with remote debugging on, and talks the Chrome DevTools Protocol to the debug
//! proxy the app's dev server hosts (`/_framework/debug/ws-proxy`), which adds the .NET side: C# and
//! Razor breakpoints, .NET call stacks, locals and expression evaluation.
//!
//! `launch` arguments: `project` (the .csproj, built already), optional `launchProfile`, `env`
//! (object), `browser` (path), `headless` (bool, for tests).

mod cdp;
pub mod launch;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use bf_lsp::framing::{FrameReader, encode};
use serde_json::{Value, json};

use cdp::{Cdp, CdpEvent};
use launch::Browser;

/// The one thread the IDE sees (the browser's .NET runtime is single-threaded).
const THREAD: i64 = 1;

enum Input {
    Dap(Value),
    Cdp(CdpEvent),
    App(&'static str, String),
    StdinClosed,
}

/// Runs the adapter on this process's stdin and stdout until the IDE disconnects.
pub fn run_stdio() {
    let (tx, rx) = flume::unbounded();
    let reader = tx.clone();
    let _ = std::thread::Builder::new().name("bf-wasm-dap-in".into()).spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut frames = FrameReader::default();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            frames.push(&buf[..n]);
            while let Ok(Some(body)) = frames.next_frame() {
                if let Ok(message) = serde_json::from_slice::<Value>(&body) {
                    let _ = reader.send(Input::Dap(message));
                }
            }
        }
        let _ = reader.send(Input::StdinClosed);
    });
    let mut adapter = Adapter::new(tx, Box::new(std::io::stdout()));
    while let Ok(input) = rx.recv() {
        if !adapter.handle(input) {
            break;
        }
    }
    adapter.shutdown();
}

struct Breakpoints {
    /// CDP breakpoint ids of each file's breakpoints, with the DAP ids they were reported under.
    by_file: HashMap<PathBuf, Vec<(String, i64)>>,
    next_id: i64,
}

struct Adapter {
    inbox: flume::Sender<Input>,
    out: Box<dyn Write + Send>,
    seq: i64,
    app: Option<Child>,
    /// Microsoft's WebAssembly debug proxy (BrowserDebugHost).
    proxy: Option<Child>,
    browser: Option<Browser>,
    cdp: Option<Cdp>,
    url: Option<String>,
    breakpoints: Breakpoints,
    /// The .NET frames of the current stop.
    frames: Vec<Value>,
    /// variablesReference → CDP object id, valid while stopped.
    objects: HashMap<i64, String>,
    stepping: bool,
    ended: bool,
}

impl Adapter {
    fn new(inbox: flume::Sender<Input>, out: Box<dyn Write + Send>) -> Adapter {
        Adapter {
            inbox,
            out,
            seq: 1,
            app: None,
            proxy: None,
            browser: None,
            cdp: None,
            url: None,
            breakpoints: Breakpoints { by_file: HashMap::new(), next_id: 1 },
            frames: Vec::new(),
            objects: HashMap::new(),
            stepping: false,
            ended: false,
        }
    }

    fn write(&mut self, mut message: Value) {
        message["seq"] = json!(self.seq);
        self.seq += 1;
        let body = message.to_string();
        let _ = self.out.write_all(&encode(body.as_bytes())).and_then(|_| self.out.flush());
    }

    fn event(&mut self, event: &str, body: Value) {
        self.write(json!({ "type": "event", "event": event, "body": body }));
    }

    fn output(&mut self, category: &str, text: impl Into<String>) {
        let mut text = text.into();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.event("output", json!({ "category": category, "output": text }));
    }

    /// Handles one input; false ends the adapter.
    fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Dap(message) if message.get("type").and_then(Value::as_str) == Some("request") => {
                let command = message.get("command").and_then(Value::as_str).unwrap_or_default().to_owned();
                let seq = message.get("seq").and_then(Value::as_i64).unwrap_or(0);
                let arguments = message.get("arguments").cloned().unwrap_or(Value::Null);
                let result = self.request(&command, &arguments);
                if command == "launch"
                    && let Err(e) = &result
                {
                    self.output("stderr", format!("Could not start debugging: {e:#}"));
                }
                let response = match result {
                    Ok(body) => json!({ "type": "response", "request_seq": seq, "command": command, "success": true, "body": body }),
                    Err(e) => json!({ "type": "response", "request_seq": seq, "command": command, "success": false, "message": format!("{e:#}") }),
                };
                self.write(response);
                if command == "launch" && self.cdp.is_some() {
                    // Breakpoints and exception settings come next; the page opens on configurationDone.
                    self.event("initialized", json!({}));
                }
                !(command == "disconnect" || command == "terminate")
            }
            Input::Dap(_) => true,
            Input::Cdp(CdpEvent::Event { method, params }) => {
                self.cdp_event(&method, &params);
                true
            }
            Input::Cdp(CdpEvent::Closed(reason)) => {
                if !self.ended {
                    self.output("console", format!("The browser closed ({reason})."));
                    self.end();
                }
                true
            }
            // The proxy's own log: only its problems are worth showing.
            Input::App("proxy", line) => {
                if line.contains("rror") || line.contains("xception") {
                    self.output("stderr", format!("[debug proxy] {line}"));
                }
                true
            }
            Input::App(stream, line) => {
                let category = if stream == "stderr" { "stderr" } else { "stdout" };
                self.output(category, line);
                true
            }
            Input::StdinClosed => false,
        }
    }

    fn request(&mut self, command: &str, arguments: &Value) -> Result<Value> {
        match command {
            "initialize" => Ok(json!({
                "supportsConfigurationDoneRequest": true,
                "supportsConditionalBreakpoints": true,
                "supportsEvaluateForHovers": true,
                "supportsTerminateRequest": true,
                "exceptionBreakpointFilters": [
                    { "filter": "all", "label": "All Exceptions", "default": false },
                    { "filter": "user-unhandled", "label": "Uncaught Exceptions", "default": true },
                ],
            })),
            "launch" => self.launch(arguments).map(|()| Value::Null),
            "attach" => bail!("Blazor WebAssembly apps are started by the debugger (launch), not attached to"),
            "configurationDone" => {
                let (cdp, url) = (self.cdp()?, self.url.clone().context("not launched")?);
                cdp.call("Page.navigate", json!({ "url": url }))?;
                Ok(Value::Null)
            }
            "setBreakpoints" => self.set_breakpoints(arguments),
            "setExceptionBreakpoints" => {
                let filters: Vec<&str> = arguments.get("filters").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                let state = if filters.contains(&"all") { "all" } else if filters.is_empty() { "none" } else { "uncaught" };
                self.cdp()?.call("Debugger.setPauseOnExceptions", json!({ "state": state }))?;
                Ok(Value::Null)
            }
            "threads" => Ok(json!({ "threads": [{ "id": THREAD, "name": "Browser" }] })),
            "stackTrace" => Ok(self.stack_trace()),
            "scopes" => self.scopes(arguments),
            "variables" => self.variables(arguments),
            "evaluate" => self.evaluate(arguments),
            "continue" => self.resume("Debugger.resume", false).map(|()| json!({ "allThreadsContinued": true })),
            "next" => self.resume("Debugger.stepOver", true).map(|()| Value::Null),
            "stepIn" => self.resume("Debugger.stepInto", true).map(|()| Value::Null),
            "stepOut" => self.resume("Debugger.stepOut", true).map(|()| Value::Null),
            "pause" => {
                self.cdp()?.call("Debugger.pause", json!({}))?;
                Ok(Value::Null)
            }
            "disconnect" | "terminate" => {
                self.end();
                Ok(Value::Null)
            }
            other => bail!("{other} isn't supported for Blazor WebAssembly"),
        }
    }

    fn cdp(&self) -> Result<Cdp> {
        self.cdp.clone().ok_or_else(|| anyhow!("the browser isn't connected"))
    }

    fn launch(&mut self, arguments: &Value) -> Result<()> {
        let project = arguments.get("project").and_then(Value::as_str).map(PathBuf::from).context("launch needs \"project\"")?;
        let profile = arguments.get("launchProfile").and_then(Value::as_str);
        let env: Vec<(String, String)> = arguments
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect();
        let headless = arguments.get("headless").and_then(Value::as_bool).unwrap_or(false);
        let browser_exe = launch::find_browser(arguments.get("browser").and_then(Value::as_str))
            .context("no browser to debug in: install Edge, Chrome or Chromium, or set \"debug.browser\" to one")?;

        // The app: its output goes to the Debug Console; its address comes from "Now listening on".
        let (urls_tx, urls) = flume::unbounded::<String>();
        let inbox = self.inbox.clone();
        self.output("console", format!("Starting {}…", project.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        let app = launch::start_app(&project, profile, &env, move |stream, line| {
            if let Some(url) = launch::listening_url(&line) {
                let _ = urls_tx.send(url);
            }
            let _ = inbox.send(Input::App(stream, line));
        })?;
        self.app = Some(app);
        let url = self.wait_for_url(&urls)?;
        self.output("console", format!("Opening {url} in {}", browser_exe.file_stem().map(|n| n.to_string_lossy()).unwrap_or_default()));

        let browser = Browser::start(&browser_exe, headless)?;
        let page = page_id(&browser.devtools)?;
        // Microsoft's debug proxy sits between this adapter and the page and adds the .NET side.
        let host = launch::find_debug_host(&project)?;
        let port = launch::free_port()?;
        let inbox = self.inbox.clone();
        self.proxy = Some(launch::start_debug_host(&host, &browser.devtools, port, move |line| {
            let _ = inbox.send(Input::App("proxy", line));
        })?);
        let proxy = format!("ws://127.0.0.1:{port}/devtools/page/{page}");
        let inbox = self.inbox.clone();
        let cdp = Cdp::connect(&proxy, move |event| {
            let _ = inbox.send(Input::Cdp(event));
        })
        .with_context(|| format!("could not reach the WebAssembly debug proxy at {proxy}"))?;
        self.browser = Some(browser);
        for (method, params) in [("Runtime.enable", json!({})), ("Debugger.enable", json!({})), ("Page.enable", json!({}))] {
            cdp.call(method, params)?;
        }
        self.cdp = Some(cdp);
        self.url = Some(url);
        Ok(())
    }

    /// Waits for the app's http address (an https one only if that's all it serves).
    fn wait_for_url(&mut self, urls: &flume::Receiver<String>) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut https: Option<(String, Instant)> = None;
        loop {
            match urls.recv_timeout(Duration::from_millis(200)) {
                Ok(url) if url.starts_with("http://") => return Ok(url),
                Ok(url) => https = https.or(Some((url, Instant::now()))),
                Err(_) => {}
            }
            if let Some(app) = self.app.as_mut()
                && let Ok(Some(status)) = app.try_wait()
            {
                bail!("the app exited before it served anything ({status})");
            }
            // An http address usually follows the https one at once.
            if let Some((url, since)) = &https
                && since.elapsed() > Duration::from_secs(5)
            {
                bail!("the app only serves https ({url}); add an http address to its launch profile, the debugger connects over http");
            }
            if Instant::now() > deadline {
                bail!("the app didn't say where it listens within 2 minutes");
            }
        }
    }

    fn set_breakpoints(&mut self, arguments: &Value) -> Result<Value> {
        let path = arguments.pointer("/source/path").and_then(Value::as_str).map(PathBuf::from).context("setBreakpoints needs a source path")?;
        let cdp = self.cdp()?;
        for (id, _) in self.breakpoints.by_file.remove(&path).unwrap_or_default() {
            let _ = cdp.call("Debugger.removeBreakpoint", json!({ "breakpointId": id }));
        }
        let mut reported = Vec::new();
        let mut kept = Vec::new();
        for bp in arguments.get("breakpoints").and_then(Value::as_array).into_iter().flatten() {
            let line = bp.get("line").and_then(Value::as_i64).unwrap_or(1);
            let mut params = json!({ "lineNumber": line - 1, "columnNumber": 0, "urlRegex": path_regex(&path) });
            if let Some(condition) = bp.get("condition").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                params["condition"] = json!(condition);
            }
            let dap_id = self.breakpoints.next_id;
            self.breakpoints.next_id += 1;
            match cdp.call("Debugger.setBreakpointByUrl", params) {
                Ok(result) => {
                    let id = result.get("breakpointId").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let location = result.get("locations").and_then(Value::as_array).and_then(|l| l.first());
                    let at = location.and_then(|l| l.get("lineNumber")).and_then(Value::as_i64).map_or(line, |l| l + 1);
                    // Until the app's assemblies load, the proxy can't place it yet (it reports it once it can).
                    reported.push(json!({ "id": dap_id, "verified": location.is_some(), "line": at, "source": { "path": path } }));
                    kept.push((id, dap_id));
                }
                Err(e) => reported.push(json!({ "id": dap_id, "verified": false, "line": line, "message": e.to_string() })),
            }
        }
        self.breakpoints.by_file.insert(path, kept);
        Ok(json!({ "breakpoints": reported }))
    }

    fn resume(&mut self, method: &str, step: bool) -> Result<()> {
        self.stepping = step;
        self.frames.clear();
        self.objects.clear();
        self.cdp()?.call(method, json!({}))?;
        Ok(())
    }

    fn stack_trace(&self) -> Value {
        let frames: Vec<Value> = self
            .frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let line = f.pointer("/location/lineNumber").and_then(Value::as_i64).unwrap_or(0) + 1;
                let column = f.pointer("/location/columnNumber").and_then(Value::as_i64).unwrap_or(0) + 1;
                let mut frame = json!({ "id": i as i64 + 1, "name": f.get("functionName").and_then(Value::as_str).unwrap_or("?"), "line": line, "column": column });
                if let Some(path) = f.get("url").and_then(Value::as_str).and_then(file_url_to_path) {
                    frame["source"] = json!({ "name": path.file_name().map(|n| n.to_string_lossy().into_owned()), "path": path });
                }
                frame
            })
            .collect();
        json!({ "stackFrames": frames, "totalFrames": frames.len() })
    }

    fn frame(&self, frame_id: i64) -> Result<&Value> {
        self.frames.get((frame_id - 1).max(0) as usize).context("not stopped there")
    }

    fn scopes(&mut self, arguments: &Value) -> Result<Value> {
        let frame = self.frame(arguments.get("frameId").and_then(Value::as_i64).unwrap_or(1))?;
        let objects: Vec<(String, String)> = frame
            .get("scopeChain")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| Some((s.get("type")?.as_str()?.to_owned(), s.pointer("/object/objectId")?.as_str()?.to_owned())))
            .collect();
        let scopes: Vec<Value> = objects
            .into_iter()
            .map(|(kind, object)| {
                let reference = self.reference(object);
                json!({ "name": if kind == "local" { "Locals".to_owned() } else { kind }, "variablesReference": reference, "expensive": false })
            })
            .collect();
        Ok(json!({ "scopes": scopes }))
    }

    fn reference(&mut self, object: String) -> i64 {
        let id = self.objects.len() as i64 + 1;
        self.objects.insert(id, object);
        id
    }

    fn variables(&mut self, arguments: &Value) -> Result<Value> {
        let reference = arguments.get("variablesReference").and_then(Value::as_i64).unwrap_or(0);
        let object = self.objects.get(&reference).cloned().context("that value is gone (the program went on)")?;
        let result = self.cdp()?.call("Runtime.getProperties", json!({ "objectId": object, "ownProperties": true }))?;
        let mut variables = Vec::new();
        for property in result.get("result").and_then(Value::as_array).into_iter().flatten() {
            let (Some(name), Some(value)) = (property.get("name").and_then(Value::as_str), property.get("value")) else { continue };
            let (text, type_name, child) = describe(value);
            let reference = child.map_or(0, |o| self.reference(o));
            variables.push(json!({ "name": name, "value": text, "type": type_name, "variablesReference": reference }));
        }
        Ok(json!({ "variables": variables }))
    }

    fn evaluate(&mut self, arguments: &Value) -> Result<Value> {
        let expression = arguments.get("expression").and_then(Value::as_str).unwrap_or_default();
        let frame_id = arguments.get("frameId").and_then(Value::as_i64).unwrap_or(1);
        let call_frame = self.frame(frame_id).map_err(|_| anyhow!("expressions are evaluated while the program is paused"))?;
        let call_frame_id = call_frame.get("callFrameId").cloned().unwrap_or(Value::Null);
        let result = self.cdp()?.call("Debugger.evaluateOnCallFrame", json!({ "callFrameId": call_frame_id, "expression": expression }))?;
        if let Some(details) = result.get("exceptionDetails") {
            let text = details.pointer("/exception/description").or_else(|| details.get("text")).and_then(Value::as_str).unwrap_or("error");
            bail!("{text}");
        }
        let (text, type_name, child) = describe(result.get("result").unwrap_or(&Value::Null));
        let reference = child.map_or(0, |o| self.reference(o));
        Ok(json!({ "result": text, "type": type_name, "variablesReference": reference }))
    }

    fn cdp_event(&mut self, method: &str, params: &Value) {
        match method {
            "Debugger.paused" => {
                let frames: Vec<Value> = params
                    .get("callFrames")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|f| f.get("callFrameId").and_then(Value::as_str).is_some_and(|id| id.starts_with("dotnet:")))
                    .cloned()
                    .collect();
                if frames.is_empty() {
                    // A JavaScript stop (the page's own scripts): not this debugger's business.
                    if let Some(cdp) = self.cdp.clone() {
                        let _ = cdp.send("Debugger.resume", json!({}));
                    }
                    return;
                }
                self.frames = frames;
                self.objects.clear();
                let hit = params.get("hitBreakpoints").and_then(Value::as_array).is_some_and(|b| !b.is_empty());
                let reason = match params.get("reason").and_then(Value::as_str) {
                    Some("exception") => "exception",
                    _ if hit => "breakpoint",
                    _ if self.stepping => "step",
                    _ => "pause",
                };
                let mut body = json!({ "reason": reason, "threadId": THREAD, "allThreadsStopped": true });
                if reason == "exception"
                    && let Some(text) = params.pointer("/data/description").or_else(|| params.pointer("/data/className")).and_then(Value::as_str)
                {
                    body["description"] = json!("Exception");
                    body["text"] = json!(text);
                }
                self.event("stopped", body);
            }
            "Debugger.resumed" => {
                self.frames.clear();
                self.objects.clear();
                self.event("continued", json!({ "threadId": THREAD, "allThreadsContinued": true }));
            }
            "Debugger.breakpointResolved" => {
                let Some(cdp_id) = params.get("breakpointId").and_then(Value::as_str) else { return };
                let found = self.breakpoints.by_file.iter().find_map(|(path, list)| list.iter().find(|(id, _)| id == cdp_id).map(|(_, dap)| (path.clone(), *dap)));
                if let Some((path, dap_id)) = found {
                    let line = params.pointer("/location/lineNumber").and_then(Value::as_i64).unwrap_or(0) + 1;
                    self.event("breakpoint", json!({ "reason": "changed", "breakpoint": { "id": dap_id, "verified": true, "line": line, "source": { "path": path } } }));
                }
            }
            "Runtime.consoleAPICalled" => {
                let text: Vec<String> = params
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|a| a.get("value").map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).or_else(|| a.get("description").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default())
                    .collect();
                let category = if params.get("type").and_then(Value::as_str) == Some("error") { "stderr" } else { "stdout" };
                self.output(category, text.join(" "));
            }
            "Runtime.exceptionThrown" => {
                if let Some(text) = params.pointer("/exceptionDetails/exception/description").or_else(|| params.pointer("/exceptionDetails/text")).and_then(Value::as_str) {
                    self.output("stderr", text.to_owned());
                }
            }
            "Inspector.detached" | "Target.targetDestroyed" if !self.ended => {
                self.output("console", "The browser page closed.");
                self.end();
            }
            _ => {}
        }
    }

    /// Ends the session: closes the browser and stops the app; tells the IDE once.
    fn end(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.stop_processes();
        self.event("exited", json!({ "exitCode": 0 }));
        self.event("terminated", json!({}));
    }

    fn shutdown(&mut self) {
        self.stop_processes();
    }

    fn stop_processes(&mut self) {
        self.cdp = None;
        for child in [self.proxy.take(), self.app.take()].into_iter().flatten() {
            let mut child = child;
            launch::kill_tree(&mut child);
        }
        self.browser = None;
    }
}

/// The id of the browser's page (the proxy connects to it on the IDE's behalf).
fn page_id(devtools: &str) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        // The port file can appear a moment before the browser answers on it.
        let answer = cdp::http_get(devtools, "/json/list");
        if let Err(e) = &answer
            && Instant::now() > deadline
        {
            bail!("the browser's DevTools at {devtools} don't answer: {e:#}");
        }
        let targets: Value = answer.ok().and_then(|body| serde_json::from_str(&body).ok()).unwrap_or(Value::Null);
        let page = targets
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|t| t.get("id").and_then(Value::as_str));
        if let Some(page) = page {
            return Ok(page.to_owned());
        }
        if Instant::now() > deadline {
            bail!("the browser has no page to debug");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A regular expression (the proxy is .NET, so .NET syntax) matching `path` as the app's debug symbols
/// name it: either separator, and without case on Windows and macOS.
fn path_regex(path: &Path) -> String {
    let mut regex = String::from(if cfg!(any(windows, target_os = "macos")) { "(?i)^" } else { "^" });
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' | '/' => regex.push_str(r"[\\/]"),
            '.' | '$' | '^' | '{' | '[' | '(' | '|' | ')' | '*' | '+' | '?' | '}' | ']' | '#' | ' ' => {
                regex.push('\\');
                regex.push(c);
            }
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex
}

/// `file:///C:/x%20y/A.razor` → `C:\x y\A.razor` (`file:///src/A.razor` → `/src/A.razor` on Unix).
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let mut bytes = Vec::new();
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            bytes.push(b);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    // "/C:/x" is a Windows path with a drive letter.
    let is_drive = decoded.len() > 2 && decoded.as_bytes()[0] == b'/' && decoded.as_bytes()[2] == b':';
    Some(if is_drive { PathBuf::from(decoded[1..].replace('/', "\\")) } else { PathBuf::from(decoded) })
}

/// A CDP value for the Variables view: its text, its type and, for objects, the object to expand.
fn describe(value: &Value) -> (String, Option<String>, Option<String>) {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let type_name = value.get("className").and_then(Value::as_str).map(str::to_owned).or_else(|| (!kind.is_empty()).then(|| kind.to_owned()));
    if value.get("subtype").and_then(Value::as_str) == Some("null") || (kind == "object" && value.get("value").is_some_and(Value::is_null) && value.get("objectId").is_none()) {
        return ("null".into(), type_name, None);
    }
    let text = match (kind, value.get("value")) {
        ("string", Some(Value::String(s))) => format!("\"{s}\""),
        (_, Some(Value::String(s))) if kind != "object" => s.clone(),
        (_, Some(v)) if !v.is_null() && !v.is_object() => v.to_string(),
        _ => value.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
    };
    let child = (kind == "object").then(|| value.get("objectId").and_then(Value::as_str).map(str::to_owned)).flatten();
    (text, type_name, child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoint_paths_become_regexes() {
        let regex = path_regex(Path::new("/src/My App/Pages/Counter.razor"));
        assert!(regex.ends_with(r"[\\/]src[\\/]My\ App[\\/]Pages[\\/]Counter\.razor$"), "{regex}");
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(file_url_to_path("file:///C:/My%20App/Pages/Counter.razor"), Some(PathBuf::from(r"C:\My App\Pages\Counter.razor")));
        assert_eq!(file_url_to_path("file:///src/App/Program.cs"), Some(PathBuf::from("/src/App/Program.cs")));
        assert_eq!(file_url_to_path("dotnet://App.dll/Program.cs"), None);
    }

    #[test]
    fn values_read_like_csharp() {
        assert_eq!(describe(&json!({ "type": "number", "value": 3, "description": "3", "className": "System.Int32" })).0, "3");
        assert_eq!(describe(&json!({ "type": "string", "value": "hi" })).0, "\"hi\"");
        let (text, ty, child) = describe(&json!({ "type": "object", "className": "List<int>", "description": "Count = 2", "objectId": "dotnet:object:4" }));
        assert_eq!((text.as_str(), ty.as_deref(), child.as_deref()), ("Count = 2", Some("List<int>"), Some("dotnet:object:4")));
        assert_eq!(describe(&json!({ "type": "object", "subtype": "null", "value": null, "className": "Foo" })).0, "null");
    }
}
