//! A small Chrome DevTools Protocol client over a local `ws://` WebSocket: one thread owns the socket,
//! writes queued commands and reads responses (matched by id) and events.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tungstenite::Message;

/// What the connection delivers besides responses.
#[derive(Debug)]
pub enum CdpEvent {
    Event { method: String, params: Value },
    /// The socket closed (the browser or the page went away).
    Closed(String),
}

type Pending = Arc<Mutex<HashMap<i64, flume::Sender<Result<Value>>>>>;

#[derive(Clone)]
pub struct Cdp {
    commands: flume::Sender<Value>,
    pending: Pending,
    next_id: Arc<AtomicI64>,
}

impl Cdp {
    /// Connects to `url` (`ws://host:port/path?query`). Events go to `events`.
    pub fn connect(url: &str, events: impl Fn(CdpEvent) + Send + 'static) -> Result<Cdp> {
        let rest = url.strip_prefix("ws://").ok_or_else(|| anyhow!("only ws:// addresses are supported: {url}"))?;
        let host = rest.split('/').next().unwrap_or(rest);
        let address = std::net::ToSocketAddrs::to_socket_addrs(host)?.next().with_context(|| format!("no address for {host}"))?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10)).with_context(|| format!("connecting to {host}"))?;
        stream.set_nodelay(true).ok();
        // A wrong endpoint must fail, not hang the handshake.
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        let (mut socket, _) = tungstenite::client(url, stream).map_err(|e| anyhow!("WebSocket handshake with {url} failed: {e}"))?;
        // Short reads let the same thread send queued commands in between.
        socket.get_mut().set_read_timeout(Some(Duration::from_millis(15)))?;
        let (commands, queue) = flume::unbounded::<Value>();
        let pending: Pending = Arc::default();
        let reader_pending = pending.clone();
        std::thread::Builder::new().name("bf-cdp".into()).spawn(move || {
            let reason = loop {
                while let Ok(command) = queue.try_recv() {
                    log::debug!("cdp → {command}");
                    if let Err(e) = socket.send(Message::text(command.to_string())) {
                        break_with(&reader_pending, &e.to_string());
                        events(CdpEvent::Closed(e.to_string()));
                        return;
                    }
                }
                match socket.read() {
                    Ok(Message::Text(text)) => dispatch(&text, &reader_pending, &events),
                    Ok(Message::Close(_)) => break "the debug connection closed".to_owned(),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(e) => break e.to_string(),
                }
                if queue.is_disconnected() && reader_pending.lock().is_empty() {
                    break "closed".to_owned();
                }
            };
            break_with(&reader_pending, &reason);
            events(CdpEvent::Closed(reason));
        })?;
        Ok(Cdp { commands, pending, next_id: Arc::new(AtomicI64::new(1)) })
    }

    /// Sends a command; the receiver gets its `result` (or the protocol error).
    pub fn send(&self, method: &str, params: Value) -> flume::Receiver<Result<Value>> {
        let (tx, rx) = flume::bounded(1);
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().insert(id, tx);
        if self.commands.send(json!({ "id": id, "method": method, "params": params })).is_err()
            && let Some(tx) = self.pending.lock().remove(&id)
        {
            let _ = tx.send(Err(anyhow!("the debug connection is closed")));
        }
        rx
    }

    /// Sends a command and waits for its result.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.send(method, params)
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| anyhow!("{method}: no answer from the browser"))?
    }
}

fn dispatch(text: &str, pending: &Pending, events: &impl Fn(CdpEvent)) {
    let Ok(message) = serde_json::from_str::<Value>(text) else { return };
    log::debug!("cdp ← {}", text.chars().take(2000).collect::<String>());
    if let Some(id) = message.get("id").and_then(Value::as_i64) {
        let Some(tx) = pending.lock().remove(&id) else { return };
        let result = match message.get("error") {
            Some(error) => Err(anyhow!("{}", error.get("message").and_then(Value::as_str).unwrap_or("request failed"))),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(result);
    } else if let Some(method) = message.get("method").and_then(Value::as_str) {
        events(CdpEvent::Event { method: method.to_owned(), params: message.get("params").cloned().unwrap_or(Value::Null) });
    }
}

fn break_with(pending: &Pending, reason: &str) {
    for (_, tx) in pending.lock().drain() {
        let _ = tx.send(Err(anyhow!("{reason}")));
    }
}

/// A plain HTTP GET on localhost (the browser's `/json/list`); returns the body.
pub fn http_get(host_port: &str, path: &str) -> Result<String> {
    let mut stream = TcpStream::connect(host_port).with_context(|| format!("connecting to {host_port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n")?;
    // Browsers keep the connection open anyway, so read the headers, then exactly Content-Length bytes.
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            bail!("{path}: the connection closed without an answer");
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        bail!("{path}: {}", head.lines().next().unwrap_or_default());
    }
    let length = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok()));
    let mut body = raw[header_end..].to_vec();
    match length {
        Some(length) => {
            while body.len() < length {
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n]);
            }
            body.truncate(length);
        }
        None => {
            let _ = stream.read_to_end(&mut body);
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

