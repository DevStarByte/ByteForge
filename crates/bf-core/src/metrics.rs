//! Live metrics of a running .NET process: `dotnet-counters collect --format json` writes the counters of
//! the process to a file once a second; [`CountersSession`] reads that file as it grows and turns the
//! counters into [`Sample`]s (CPU, working set, GC heap, GC count, thread pool queue, exceptions, ASP.NET
//! Core requests per second).
//!
//! Both counter schemes are understood: the EventCounters of .NET 8 and older ("CPU Usage (%)",
//! "Working Set (MB)", …) and the `System.Runtime` meter of .NET 9 and newer (`dotnet.process.cpu.time`,
//! `dotnet.process.memory.working_set`, …).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;

use crate::debug::RuntimeProcess;

/// The counter providers asked for.
pub const PROVIDERS: &str = "System.Runtime,Microsoft.AspNetCore.Hosting";

/// What the Metrics tab shows, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    Cpu,
    WorkingSet,
    GcHeap,
    GcCount,
    ThreadPoolQueue,
    Exceptions,
    Requests,
}

impl Metric {
    pub const ALL: [Metric; 7] = [Metric::Cpu, Metric::WorkingSet, Metric::GcHeap, Metric::GcCount, Metric::ThreadPoolQueue, Metric::Exceptions, Metric::Requests];

    pub fn label(self) -> &'static str {
        match self {
            Metric::Cpu => "CPU",
            Metric::WorkingSet => "Working set",
            Metric::GcHeap => "GC heap",
            Metric::GcCount => "GC collections",
            Metric::ThreadPoolQueue => "Thread pool queue",
            Metric::Exceptions => "Exceptions",
            Metric::Requests => "Requests",
        }
    }

    /// The value as shown: "12.5%", "148.2 MB", "3/s", "17".
    pub fn format(self, value: f64) -> String {
        match self {
            Metric::Cpu => format!("{value:.1}%"),
            Metric::WorkingSet | Metric::GcHeap => format_bytes(value),
            Metric::GcCount | Metric::Exceptions | Metric::Requests => format!("{}/s", trim_number(value)),
            Metric::ThreadPoolQueue => trim_number(value),
        }
    }

    fn index(self) -> usize {
        Metric::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

fn trim_number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 { format!("{value:.0}") } else { format!("{value:.1}") }
}

/// "812 KB", "148.2 MB", "1.25 GB" (powers of 1024).
pub fn format_bytes(bytes: f64) -> String {
    const K: f64 = 1024.0;
    if bytes < K * K {
        format!("{:.0} KB", bytes / K)
    } else if bytes < K * K * K {
        format!("{:.1} MB", bytes / (K * K))
    } else {
        format!("{:.2} GB", bytes / (K * K * K))
    }
}

/// The counters of one second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    values: [Option<f64>; 7],
}

impl Sample {
    pub fn get(&self, metric: Metric) -> Option<f64> {
        self.values[metric.index()]
    }

    fn set(&mut self, metric: Metric, value: f64) {
        self.values[metric.index()] = Some(value);
    }

    fn add(&mut self, metric: Metric, value: f64) {
        let slot = &mut self.values[metric.index()];
        *slot = Some(slot.unwrap_or(0.0) + value);
    }

    pub fn is_empty(&self) -> bool {
        self.values.iter().all(Option::is_none)
    }
}

/// One counter value as dotnet-counters writes it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CounterEvent {
    #[serde(default)]
    pub timestamp: String,
    pub name: String,
    #[serde(default)]
    pub tags: String,
    pub value: f64,
}

/// How a counter becomes a metric: a value that replaces, or one of several that add up (per GC
/// generation, per exception type), already scaled.
enum Contribution {
    Set(Metric, f64),
    Add(Metric, f64),
}

/// Maps a counter (`name` as dotnet-counters shows it, with its unit in parentheses) to a metric.
/// `cpus` turns the CPU time of the newer runtime into a percentage like the older "CPU Usage (%)".
fn classify(name: &str, value: f64, cpus: f64) -> Option<Contribution> {
    let lower = name.to_ascii_lowercase();
    let base = lower.split(" (").next().unwrap_or_default().trim();
    const MB: f64 = 1_000_000.0;
    Some(match base {
        "cpu usage" => Contribution::Set(Metric::Cpu, value),
        "dotnet.process.cpu.time" => Contribution::Set(Metric::Cpu, value / cpus.max(1.0) * 100.0),
        "working set" => Contribution::Set(Metric::WorkingSet, value * MB),
        "dotnet.process.memory.working_set" => Contribution::Set(Metric::WorkingSet, value),
        "gc heap size" => Contribution::Set(Metric::GcHeap, value * MB),
        "dotnet.gc.last_collection.heap.size" => Contribution::Add(Metric::GcHeap, value),
        "gen 0 gc count" | "gen 1 gc count" | "gen 2 gc count" | "dotnet.gc.collections" => Contribution::Add(Metric::GcCount, value),
        "threadpool queue length" | "dotnet.thread_pool.queue.length" => Contribution::Set(Metric::ThreadPoolQueue, value),
        "exception count" | "dotnet.exceptions" => Contribution::Add(Metric::Exceptions, value),
        "request rate" | "requests-per-second" => Contribution::Set(Metric::Requests, value),
        _ => return None,
    })
}

/// Groups counter events into one [`Sample`] per timestamp (dotnet-counters writes all counters of a
/// second with the same timestamp).
pub struct Aggregator {
    cpus: f64,
    timestamp: Option<String>,
    current: Sample,
}

impl Aggregator {
    pub fn new(cpus: f64) -> Self {
        Self { cpus, timestamp: None, current: Sample::default() }
    }

    /// Adds an event; returns the previous second's sample once the events of a new second start.
    pub fn push(&mut self, event: &CounterEvent) -> Option<Sample> {
        let mut done = None;
        if self.timestamp.as_deref() != Some(event.timestamp.as_str()) {
            done = self.flush();
            self.timestamp = Some(event.timestamp.clone());
        }
        if event.value.is_finite() {
            match classify(&event.name, event.value, self.cpus) {
                Some(Contribution::Set(m, v)) => self.current.set(m, v),
                Some(Contribution::Add(m, v)) => self.current.add(m, v),
                None => {}
            }
        }
        done
    }

    /// The sample being collected, if it has anything.
    pub fn flush(&mut self) -> Option<Sample> {
        let sample = std::mem::take(&mut self.current);
        (!sample.is_empty()).then_some(sample)
    }
}

/// Reads the counter events out of the JSON file dotnet-counters is still writing
/// (`{"TargetProcess": …, "Events": [ {…}, {…}, …` without its end): each object one level inside the
/// outer one is an event, complete once its brace closes. Fed the file in pieces as it grows.
#[derive(Default)]
pub struct JsonTail {
    buf: String,
    /// How far `buf` was scanned.
    pos: usize,
    depth: usize,
    in_string: bool,
    escaped: bool,
    /// Where the event being read starts in `buf`.
    start: Option<usize>,
}

impl JsonTail {
    pub fn feed(&mut self, chunk: &str) -> Vec<CounterEvent> {
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        let bytes = self.buf.as_bytes();
        let mut i = self.pos;
        while i < bytes.len() {
            let b = bytes[i];
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if b == b'\\' {
                    self.escaped = true;
                } else if b == b'"' {
                    self.in_string = false;
                }
            } else {
                match b {
                    b'"' => self.in_string = true,
                    b'{' => {
                        self.depth += 1;
                        if self.depth == 2 {
                            self.start = Some(i);
                        }
                    }
                    b'}' => {
                        if self.depth == 2
                            && let Some(start) = self.start.take()
                            && let Ok(event) = serde_json::from_str::<CounterEvent>(&self.buf[start..=i])
                        {
                            events.push(event);
                        }
                        self.depth = self.depth.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        // Keep only what an unfinished event still needs.
        let keep = self.start.unwrap_or(i);
        self.buf.drain(..keep);
        self.pos = i - keep;
        if let Some(start) = self.start.as_mut() {
            *start -= keep;
        }
        events
    }
}

/// The `dotnet-counters` tool: on PATH, or where `dotnet tool install --global` puts it.
pub fn counters_tool() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "dotnet-counters.exe" } else { "dotnet-counters" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".dotnet").join("tools"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// The arguments of `dotnet-counters` that write the counters of `pid` to `out` once a second.
pub fn collect_args(pid: u32, out: &Path) -> Vec<String> {
    vec![
        "collect".into(),
        "--process-id".into(),
        pid.to_string(),
        "--refresh-interval".into(),
        "1".into(),
        "--counters".into(),
        PROVIDERS.into(),
        "--format".into(),
        "json".into(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
    ]
}

/// The process of the app being run or debugged, among the running .NET processes: one whose
/// executable or dll is named like the project (`App`, `App.dll`, `App.exe`), and not the `dotnet run`,
/// `dotnet watch` or build processes around it.
pub fn app_process(processes: &[RuntimeProcess], app: &str) -> Option<u32> {
    let app = app.to_ascii_lowercase();
    // Linux keeps only the first 15 bytes of a process name.
    let short: String = app.chars().take(15).collect();
    processes
        .iter()
        .filter(|p| {
            let args: Vec<&str> = p.command_line.split_whitespace().collect();
            let tooling = args.iter().any(|a| matches!(*a, "run" | "watch" | "build" | "test" | "collect" | "monitor"))
                || p.command_line.contains("MSBuild")
                || p.command_line.contains("dotnet-counters")
                || p.command_line.contains("dotnet-watch");
            if tooling {
                return false;
            }
            let named = |a: &str| {
                let stem = Path::new(a).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                let ext = Path::new(a).extension().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                stem == app && matches!(ext.as_str(), "" | "dll" | "exe")
            };
            p.name.to_ascii_lowercase() == short || p.name.to_ascii_lowercase().trim_end_matches(".exe") == app || args.iter().take(2).any(|a| named(a))
        })
        .map(|p| p.pid)
        .max()
}

/// What a [`CountersSession`] reports.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricsEvent {
    Sample(Sample),
    /// dotnet-counters ended (the process exited, or it couldn't attach): its last words.
    Ended(String),
}

/// A running `dotnet-counters collect` for one process, with a thread reading its file.
pub struct CountersSession {
    child: Arc<Mutex<Option<Child>>>,
}

impl CountersSession {
    /// Starts dotnet-counters (`tool`) for `pid`; samples arrive on the channel once a second.
    pub fn start(tool: &Path, pid: u32) -> anyhow::Result<(Self, flume::Receiver<MetricsEvent>)> {
        let file = std::env::temp_dir().join(format!("byteforge-counters-{}-{pid}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut child = crate::process::command(tool)
            .args(collect_args(pid, &file))
            .env("DOTNET_CLI_UI_LANGUAGE", "en")
            .env("DOTNET_NOLOGO", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Its console output only matters when it ends early (why it couldn't attach).
        let said = Arc::new(Mutex::new(String::new()));
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as _)].into_iter().flatten() {
            let said = said.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
                    let line = line.trim().to_owned();
                    if !line.is_empty() {
                        *said.lock() = line;
                    }
                }
            });
        }
        let child = Arc::new(Mutex::new(Some(child)));
        let (tx, rx) = flume::unbounded();
        let watcher = child.clone();
        let cpus = std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64);
        std::thread::spawn(move || {
            let mut tail = JsonTail::default();
            let mut aggregator = Aggregator::new(cpus);
            let mut offset = 0u64;
            let mut read_new = |tail: &mut JsonTail, aggregator: &mut Aggregator| -> bool {
                let Ok(mut f) = std::fs::File::open(&file) else { return true };
                if f.seek(SeekFrom::Start(offset)).is_err() {
                    return true;
                }
                let mut bytes = Vec::new();
                if f.read_to_end(&mut bytes).is_err() {
                    return true;
                }
                // Hold back an incomplete UTF-8 sequence at the end for the next read.
                let valid = match std::str::from_utf8(&bytes) {
                    Ok(_) => bytes.len(),
                    Err(e) => e.valid_up_to(),
                };
                offset += valid as u64;
                let text = String::from_utf8_lossy(&bytes[..valid]);
                for event in tail.feed(&text) {
                    if let Some(sample) = aggregator.push(&event)
                        && tx.send(MetricsEvent::Sample(sample)).is_err()
                    {
                        return false;
                    }
                }
                true
            };
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if !read_new(&mut tail, &mut aggregator) {
                    break;
                }
                let exited = match watcher.lock().as_mut() {
                    Some(c) => !matches!(c.try_wait(), Ok(None)),
                    None => true,
                };
                if exited {
                    std::thread::sleep(Duration::from_millis(200));
                    read_new(&mut tail, &mut aggregator);
                    if let Some(sample) = aggregator.flush() {
                        let _ = tx.send(MetricsEvent::Sample(sample));
                    }
                    *watcher.lock() = None;
                    let _ = tx.send(MetricsEvent::Ended(said.lock().clone()));
                    break;
                }
            }
            let _ = std::fs::remove_file(&file);
        });
        Ok((Self { child }, rx))
    }

    /// Stops dotnet-counters (the app goes on).
    pub fn stop(&self) {
        if let Some(child) = self.child.lock().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for CountersSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ts: &str, name: &str, value: f64) -> CounterEvent {
        CounterEvent { timestamp: ts.into(), name: name.into(), tags: String::new(), value }
    }

    /// The file is read as it grows: events split across reads, a string with braces, the unclosed end.
    #[test]
    fn reads_events_from_a_growing_file() {
        let text = r#"{ "TargetProcess": "App {1}", "StartTime": "11/23/2026 10:00:00", "Events": [
{ "timestamp": "2026-11-23 10:00:01", "provider": "System.Runtime", "name": "CPU Usage (%)", "tags": "", "counterType": "Metric", "value": 12.5 },
{ "timestamp": "2026-11-23 10:00:01", "provider": "System.Runtime", "name": "Working Set (MB)", "tags": "", "counterType": "Metric", "value": 148 },
{ "timestamp": "2026-11-23 10:00:02", "provider": "System.Runtime", "name": "CPU Usage (%)", "tags": "", "counterType": "Metric", "value": 3 },"#;
        for split in [1, 40, 150, 200, text.len() - 5] {
            let mut tail = JsonTail::default();
            let mut events = tail.feed(&text[..split]);
            events.extend(tail.feed(&text[split..]));
            assert_eq!(events.len(), 3, "split at {split}");
            assert_eq!(events[1].name, "Working Set (MB)");
            assert_eq!(events[2].value, 3.0);
        }
    }

    /// The older EventCounters and the newer System.Runtime meter end up as the same metrics.
    #[test]
    fn both_counter_schemes_make_samples() {
        let mut a = Aggregator::new(4.0);
        assert!(a.push(&event("1", "CPU Usage (%)", 12.5)).is_none());
        a.push(&event("1", "Working Set (MB)", 148.0));
        a.push(&event("1", "Gen 0 GC Count (Count / 1 sec)", 2.0));
        a.push(&event("1", "Gen 1 GC Count (Count / 1 sec)", 1.0));
        a.push(&event("1", "Request Rate (Count / 1 sec)", 40.0));
        a.push(&event("1", "Something Else", 1.0));
        let old = a.push(&event("2", "dotnet.process.cpu.time (s)", 0.4)).unwrap();
        assert_eq!(old.get(Metric::Cpu), Some(12.5));
        assert_eq!(old.get(Metric::WorkingSet), Some(148_000_000.0));
        assert_eq!(old.get(Metric::GcCount), Some(3.0));
        assert_eq!(old.get(Metric::Requests), Some(40.0));
        assert_eq!(old.get(Metric::Exceptions), None);

        a.push(&event("2", "dotnet.process.memory.working_set (By)", 1024.0 * 1024.0));
        a.push(&event("2", "dotnet.gc.last_collection.heap.size (By)", 1000.0));
        a.push(&event("2", "dotnet.gc.last_collection.heap.size (By)", 500.0));
        a.push(&event("2", "dotnet.thread_pool.queue.length ({work_item})", 7.0));
        a.push(&event("2", "dotnet.exceptions ({exception})", 1.0));
        let new = a.flush().unwrap();
        assert!((new.get(Metric::Cpu).unwrap() - 10.0).abs() < 1e-9, "0.4 s of CPU in a second on 4 cores is 10%");
        assert_eq!(new.get(Metric::WorkingSet), Some(1024.0 * 1024.0));
        assert_eq!(new.get(Metric::GcHeap), Some(1500.0));
        assert_eq!(new.get(Metric::ThreadPoolQueue), Some(7.0));
        assert_eq!(new.get(Metric::Exceptions), Some(1.0));
        assert!(a.flush().is_none());
    }

    #[test]
    fn formats_values() {
        assert_eq!(Metric::Cpu.format(12.34), "12.3%");
        assert_eq!(Metric::WorkingSet.format(148.25 * 1024.0 * 1024.0), "148.2 MB");
        assert_eq!(Metric::GcCount.format(3.0), "3/s");
        assert_eq!(Metric::Requests.format(2.5), "2.5/s");
        assert_eq!(Metric::ThreadPoolQueue.format(0.0), "0");
        assert_eq!(format_bytes(512.0 * 1024.0), "512 KB");
        assert_eq!(format_bytes(1.5 * 1024.0 * 1024.0 * 1024.0), "1.50 GB");
    }

    /// The app is found among the processes, not the dotnet run / watch / build processes around it.
    #[test]
    fn finds_the_app_being_run() {
        let p = |pid, name: &str, cmd: &str| RuntimeProcess { pid, name: name.into(), command_line: cmd.into() };
        let processes = vec![
            p(10, "dotnet", "dotnet run --project /w/WebShop/WebShop.csproj"),
            p(11, "dotnet", "/usr/share/dotnet/dotnet /usr/share/dotnet/sdk/10.0/MSBuild.dll -nodereuse:false"),
            p(12, "WebShop", "/w/WebShop/bin/Debug/net10.0/WebShop"),
            p(13, "dotnet", "dotnet exec /w/Other/bin/Other.dll"),
        ];
        assert_eq!(app_process(&processes, "WebShop"), Some(12));
        // Started through dotnet: the dll names it.
        let processes = vec![p(20, "dotnet", "dotnet watch run --project WebShop.csproj"), p(21, "dotnet", "/usr/bin/dotnet /w/bin/Debug/net10.0/WebShop.dll")];
        assert_eq!(app_process(&processes, "WebShop"), Some(21));
        // Linux shortens process names to 15 bytes.
        let processes = vec![p(30, "Contoso.Invento", "/w/bin/Contoso.Inventory.Api")];
        assert_eq!(app_process(&processes, "Contoso.Inventory.Api"), Some(30));
        assert_eq!(app_process(&processes, "Shop"), None);
    }

    #[test]
    fn collect_writes_json_once_a_second() {
        let args = collect_args(42, Path::new("/tmp/c.json"));
        assert_eq!(args[..3], ["collect", "--process-id", "42"]);
        assert!(args.windows(2).any(|w| w == ["--format", "json"]));
        assert!(args.windows(2).any(|w| w == ["--refresh-interval", "1"]));
    }
}
