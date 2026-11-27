//! Profiling a running .NET process: CPU traces from `dotnet-trace` (read from its Speedscope export into
//! a call tree and per-method times) and heap snapshots from `dotnet-gcdump` (its report: bytes and
//! objects per type).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A .NET global tool: on PATH, or where `dotnet tool install --global` puts it.
pub fn global_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".dotnet").join("tools"));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

pub const TRACE_TOOL: &str = "dotnet-trace";
pub const GCDUMP_TOOL: &str = "dotnet-gcdump";

/// `dotnet-trace` arguments that trace `pid` (its default providers sample the threads' stacks) until
/// Enter is pressed, and also write a Speedscope file next to `out` (see [`speedscope_path`]).
pub fn trace_args(pid: u32, out: &Path) -> Vec<String> {
    vec![
        "collect".into(),
        "--process-id".into(),
        pid.to_string(),
        "--format".into(),
        "Speedscope".into(),
        "--output".into(),
        out.to_string_lossy().into_owned(),
    ]
}

/// Where `dotnet-trace` writes the Speedscope file of a trace written to `out` (`x.nettrace` → `x.speedscope.json`).
pub fn speedscope_path(out: &Path) -> PathBuf {
    out.with_extension("speedscope.json")
}

/// Environment for the tools: English messages and numbers grouped the same way on every machine.
pub fn tool_env() -> [(&'static str, &'static str); 3] {
    [("DOTNET_CLI_UI_LANGUAGE", "en"), ("DOTNET_NOLOGO", "1"), ("DOTNET_SYSTEM_GLOBALIZATION_INVARIANT", "1")]
}

// ───────────── CPU profiles ─────────────

/// A method as the trace names it: `Busy!Work.Slow(int32)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The whole name.
    pub name: String,
    /// The assembly (`Busy`, `System.Private.CoreLib`); empty for names without one.
    pub module: String,
    /// The method with its type and parameters (`Work.Slow(int32)`).
    pub method: String,
}

impl Frame {
    fn parse(name: &str) -> Self {
        match name.split_once('!') {
            Some((module, method)) => {
                let module = module.strip_suffix(".il").or_else(|| module.strip_suffix(".dll")).unwrap_or(module);
                Frame { name: name.to_owned(), module: module.to_owned(), method: method.to_owned() }
            }
            None => Frame { name: name.to_owned(), module: String::new(), method: name.to_owned() },
        }
    }

    /// The type and method name to look the source up by: `Work.Slow(int32)` → (`Work`, `Slow`). Generic
    /// arguments and the namespace are dropped (`Ns.List`1.Add` → `List`, `Add`).
    pub fn source_name(&self) -> Option<(String, String)> {
        let head = without_generics(self.method.split('(').next()?.trim());
        let (owner, method) = head.rsplit_once('.')?;
        let owner = owner.rsplit('.').next().unwrap_or(owner).to_owned();
        let method = method.to_owned();
        (!owner.is_empty() && !method.is_empty() && method != ".ctor" && method != ".cctor").then_some((owner, method))
    }

    /// The method without its namespace and parameters, for display: `Work.Slow`.
    pub fn short(&self) -> String {
        let head = self.method.split('(').next().unwrap_or(&self.method);
        // Keep the type and method; the namespace goes (but dots inside generic arguments stay).
        let mut depth = 0;
        let mut dots = Vec::new();
        for (i, c) in head.char_indices() {
            match c {
                '<' | '[' => depth += 1,
                '>' | ']' => depth -= 1,
                '.' if depth == 0 && !head[..i].ends_with('.') => dots.push(i),
                _ => {}
            }
        }
        match dots.len() {
            0 | 1 => head.to_owned(),
            n => head[dots[n - 2] + 1..].to_owned(),
        }
    }
}

/// A name without generic arguments and arity: ``List`1[System.__Canon].Add`` → `List.Add`.
fn without_generics(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut arity = false;
    for c in name.chars() {
        match c {
            '[' | '<' => depth += 1,
            ']' | '>' => depth = depth.saturating_sub(1),
            '`' if depth == 0 => arity = true,
            _ if depth > 0 => {}
            c if arity && c.is_ascii_digit() => {}
            c => {
                arity = false;
                out.push(c);
            }
        }
    }
    out
}

/// Time in milliseconds: running (CPU) and all of it (also waiting).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Times {
    pub cpu: f64,
    pub wall: f64,
}

impl Times {
    pub fn get(&self, cpu: bool) -> f64 {
        if cpu { self.cpu } else { self.wall }
    }

    fn add(&mut self, ms: f64, running: bool) {
        self.wall += ms;
        if running {
            self.cpu += ms;
        }
    }
}

/// A call path in the tree: the method, the time spent in it and below it, and its callees.
#[derive(Clone, Debug)]
pub struct CallNode {
    /// Index into [`Profile::frames`] (`usize::MAX` for the root).
    pub frame: usize,
    pub total: Times,
    pub own: Times,
    /// Indexes into [`Profile::nodes`].
    pub children: Vec<usize>,
}

/// A method's times over all its call paths.
#[derive(Clone, Debug)]
pub struct MethodTimes {
    pub frame: usize,
    /// Time in the method itself.
    pub own: Times,
    /// Time in it or anything it called (counted once when it's on the stack several times).
    pub total: Times,
}

/// A CPU trace read back: every thread's stacks merged into one call tree.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub frames: Vec<Frame>,
    /// `nodes[0]` is the root; its total is the whole trace.
    pub nodes: Vec<CallNode>,
    /// By own CPU time, highest first.
    pub methods: Vec<MethodTimes>,
    pub threads: usize,
    /// How long the trace ran.
    pub duration_ms: f64,
}

impl Profile {
    pub fn root(&self) -> &CallNode {
        &self.nodes[0]
    }

    pub fn frame(&self, node: &CallNode) -> Option<&Frame> {
        self.frames.get(node.frame)
    }

    /// A node's children, the most time first.
    pub fn children_sorted(&self, node: usize, cpu: bool) -> Vec<usize> {
        let mut children = self.nodes[node].children.clone();
        children.sort_by(|a, b| self.nodes[*b].total.get(cpu).total_cmp(&self.nodes[*a].total.get(cpu)));
        children
    }
}

#[derive(Deserialize)]
struct SpeedscopeFile {
    shared: Shared,
    profiles: Vec<SpeedscopeProfile>,
}

#[derive(Deserialize)]
struct Shared {
    frames: Vec<SpeedscopeFrame>,
}

#[derive(Deserialize)]
struct SpeedscopeFrame {
    name: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SpeedscopeProfile {
    Evented {
        #[serde(rename = "startValue", default)]
        start: f64,
        #[serde(rename = "endValue", default)]
        end: f64,
        #[serde(default)]
        events: Vec<SpeedscopeEvent>,
    },
    Sampled {
        #[serde(rename = "startValue", default)]
        start: f64,
        #[serde(rename = "endValue", default)]
        end: f64,
        #[serde(default)]
        samples: Vec<Vec<usize>>,
        #[serde(default)]
        weights: Vec<f64>,
    },
}

#[derive(Deserialize)]
struct SpeedscopeEvent {
    #[serde(rename = "type")]
    kind: String,
    frame: usize,
    at: f64,
}

/// What a frame name stands for in the stacks dotnet-trace exports.
#[derive(Clone, Copy, PartialEq)]
enum FrameKind {
    Method,
    /// The process, thread and activity frames at the bottom of every stack.
    Container,
    /// The state at the top of a stack: `CPU_TIME` (running .NET code), `UNMANAGED_CODE_TIME`, `BLOCKED_TIME`, ….
    State { running: bool },
}

fn kind_of(name: &str) -> FrameKind {
    if name.contains('!') {
        return FrameKind::Method;
    }
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return FrameKind::State { running: name == "CPU_TIME" };
    }
    if name.starts_with("Process") || name.starts_with("Thread") || name == "Threads" || name.starts_with("(Non-Activities)") || name.starts_with("Activity") {
        return FrameKind::Container;
    }
    FrameKind::Method
}

/// Builds the profile while the stacks are read.
struct Builder {
    kinds: Vec<FrameKind>,
    nodes: Vec<CallNode>,
    index: HashMap<(usize, usize), usize>,
    own: Vec<Times>,
    total: Vec<Times>,
    /// The interval a frame was last counted in (so recursion counts once).
    seen: Vec<u64>,
    interval: u64,
}

impl Builder {
    fn new(kinds: Vec<FrameKind>) -> Self {
        let n = kinds.len();
        Builder {
            kinds,
            nodes: vec![CallNode { frame: usize::MAX, total: Times::default(), own: Times::default(), children: Vec::new() }],
            index: HashMap::new(),
            own: vec![Times::default(); n],
            total: vec![Times::default(); n],
            seen: vec![0; n],
            interval: 0,
        }
    }

    /// `stack` (bottom first) ran for `ms`.
    fn account(&mut self, stack: &[usize], ms: f64) {
        if ms <= 0.0 {
            return;
        }
        let running = match stack.last().map(|&f| self.kinds.get(f).copied()) {
            Some(Some(FrameKind::State { running })) => running,
            // Exporters without a state frame: count it as running.
            _ => true,
        };
        self.interval += 1;
        self.nodes[0].total.add(ms, running);
        let mut node = 0;
        let mut leaf = None;
        for &f in stack {
            if self.kinds.get(f) != Some(&FrameKind::Method) {
                continue;
            }
            let next = match self.index.get(&(node, f)) {
                Some(&n) => n,
                None => {
                    let n = self.nodes.len();
                    self.nodes.push(CallNode { frame: f, total: Times::default(), own: Times::default(), children: Vec::new() });
                    self.nodes[node].children.push(n);
                    self.index.insert((node, f), n);
                    n
                }
            };
            self.nodes[next].total.add(ms, running);
            if self.seen[f] != self.interval {
                self.seen[f] = self.interval;
                self.total[f].add(ms, running);
            }
            node = next;
            leaf = Some(f);
        }
        self.nodes[node].own.add(ms, running);
        if let Some(f) = leaf {
            self.own[f].add(ms, running);
        }
    }
}

/// Reads the Speedscope file dotnet-trace writes (`--format Speedscope`).
pub fn parse_speedscope(json: &str) -> anyhow::Result<Profile> {
    let file: SpeedscopeFile = serde_json::from_str(json)?;
    let frames: Vec<Frame> = file.shared.frames.iter().map(|f| Frame::parse(&f.name)).collect();
    let mut b = Builder::new(file.shared.frames.iter().map(|f| kind_of(&f.name)).collect());
    let mut duration: f64 = 0.0;
    let threads = file.profiles.len();
    for profile in &file.profiles {
        match profile {
            SpeedscopeProfile::Evented { start, end, events } => {
                duration = duration.max(end - start);
                let mut stack: Vec<usize> = Vec::new();
                let mut last = *start;
                for e in events {
                    if !stack.is_empty() {
                        b.account(&stack, e.at - last);
                    }
                    last = e.at;
                    match e.kind.as_str() {
                        "O" => stack.push(e.frame),
                        "C" => {
                            // Close up to (and including) the frame.
                            if let Some(pos) = stack.iter().rposition(|&f| f == e.frame) {
                                stack.truncate(pos);
                            }
                        }
                        _ => {}
                    }
                }
            }
            SpeedscopeProfile::Sampled { start, end, samples, weights } => {
                duration = duration.max(end - start);
                for (i, stack) in samples.iter().enumerate() {
                    b.account(stack, weights.get(i).copied().unwrap_or(1.0));
                }
            }
        }
    }
    let mut methods: Vec<MethodTimes> = (0..frames.len())
        .filter(|&f| b.kinds[f] == FrameKind::Method && b.total[f].wall > 0.0)
        .map(|f| MethodTimes { frame: f, own: b.own[f], total: b.total[f] })
        .collect();
    methods.sort_by(|a, b| b.own.cpu.total_cmp(&a.own.cpu).then(b.total.cpu.total_cmp(&a.total.cpu)));
    Ok(Profile { frames, nodes: b.nodes, methods, threads, duration_ms: duration })
}

/// The path to follow when a call tree opens: from the root, the child with the most time while it holds
/// at least `min_share` of the whole (node indexes, root excluded).
pub fn hot_path(profile: &Profile, cpu: bool, min_share: f64) -> Vec<usize> {
    let whole = profile.root().total.get(cpu).max(f64::MIN_POSITIVE);
    let mut path = Vec::new();
    let mut node = 0;
    while let Some(&next) = profile.children_sorted(node, cpu).first() {
        if profile.nodes[next].total.get(cpu) / whole < min_share {
            break;
        }
        path.push(next);
        node = next;
    }
    path
}

// ───────────── heap snapshots ─────────────

/// One type on the heap.
#[derive(Clone, Debug, PartialEq)]
pub struct HeapType {
    pub name: String,
    pub module: String,
    pub count: u64,
    pub bytes: u64,
}

/// A heap snapshot's types (by bytes, most first) and totals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeapReport {
    pub total_bytes: u64,
    pub total_objects: u64,
    pub types: Vec<HeapType>,
}

impl HeapReport {
    pub fn get(&self, name: &str) -> Option<&HeapType> {
        self.types.iter().find(|t| t.name == name)
    }
}

/// `dotnet-gcdump collect` arguments: a snapshot of `pid`'s heap written to `out`.
pub fn gcdump_collect_args(pid: u32, out: &Path) -> Vec<String> {
    vec!["collect".into(), "--process-id".into(), pid.to_string(), "--output".into(), out.to_string_lossy().into_owned()]
}

/// `dotnet-gcdump report` arguments for a snapshot file.
pub fn gcdump_report_args(file: &Path) -> Vec<String> {
    vec!["report".into(), file.to_string_lossy().into_owned()]
}

/// `text` after its first `n` whitespace-separated tokens, trimmed.
fn after_tokens(text: &str, n: usize) -> &str {
    let mut rest = text.trim_start();
    for _ in 0..n {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = rest[end..].trim_start();
    }
    rest.trim_end()
}

fn number(token: &str) -> Option<u64> {
    let digits: String = token.chars().filter(|c| !matches!(c, ',' | '.' | '\'' | '\u{a0}' | '\u{202f}' | '_')).collect();
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
}

/// Reads `dotnet-gcdump report` (heapstat): each row is the size of one object of the type, the number of
/// them and the type (`  [Module.dll]`). Arrays come in rows by size (`Order[] (Bytes > 100K)`); they're
/// added up per type.
pub fn parse_gcdump_report(text: &str) -> HeapReport {
    let mut report = HeapReport::default();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let mut tokens = trimmed.split_whitespace();
        let (Some(first), Some(second)) = (tokens.next(), tokens.next()) else { continue };
        let Some(a) = number(first) else { continue };
        if trimmed.ends_with("GC Heap bytes") {
            report.total_bytes = a;
            continue;
        }
        if trimmed.ends_with("GC Heap objects") {
            report.total_objects = a;
            continue;
        }
        let Some(count) = number(second) else { continue };
        // The rest after the two numbers.
        let rest = after_tokens(trimmed, 2);
        let (mut name, module) = match rest.rfind("  [") {
            Some(i) if rest.ends_with(']') => (rest[..i].trim().to_owned(), rest[i + 3..rest.len() - 1].trim().to_owned()),
            _ => (rest.to_owned(), String::new()),
        };
        if let Some(i) = name.find(" (Bytes >") {
            name.truncate(i);
        }
        let module = module.strip_suffix(".dll").unwrap_or(&module).to_owned();
        let bytes = a.saturating_mul(count);
        match by_name.get(&name) {
            Some(&i) => {
                report.types[i].count += count;
                report.types[i].bytes += bytes;
            }
            None => {
                by_name.insert(name.clone(), report.types.len());
                report.types.push(HeapType { name, module, count, bytes });
            }
        }
    }
    report.types.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Like dotnet-trace's export: process/thread frames below the methods, the state on top.
    const TRACE: &str = r#"{"shared":{"frames":[
        {"name":"Process64 Program (1) Args: Busy.dll"},{"name":"(Non-Activities)"},{"name":"Threads"},{"name":"Thread (7)"},
        {"name":"Busy!Program.<Main>$(class System.String[])"},{"name":"Busy!Work.Slow(int32)"},{"name":"Busy!Work.Inner(int32)"},
        {"name":"CPU_TIME"},{"name":"BLOCKED_TIME"},{"name":"Busy!Work.Fast(int32)"}]},
      "profiles":[{"type":"evented","name":"Thread (7)","unit":"milliseconds","startValue":0,"endValue":100,"events":[
        {"type":"O","frame":0,"at":0},{"type":"O","frame":1,"at":0},{"type":"O","frame":2,"at":0},{"type":"O","frame":3,"at":0},
        {"type":"O","frame":4,"at":0},{"type":"O","frame":5,"at":0},{"type":"O","frame":7,"at":0},
        {"type":"C","frame":7,"at":10},{"type":"O","frame":6,"at":10},{"type":"O","frame":7,"at":10},
        {"type":"C","frame":7,"at":40},{"type":"C","frame":6,"at":40},{"type":"C","frame":5,"at":40},
        {"type":"O","frame":9,"at":40},{"type":"O","frame":8,"at":40},{"type":"C","frame":8,"at":60},{"type":"C","frame":9,"at":60},
        {"type":"O","frame":5,"at":60},{"type":"O","frame":5,"at":60},{"type":"O","frame":7,"at":60},
        {"type":"C","frame":7,"at":70},{"type":"C","frame":5,"at":70},{"type":"C","frame":5,"at":70},
        {"type":"C","frame":4,"at":70},{"type":"C","frame":3,"at":70},{"type":"C","frame":2,"at":70},{"type":"C","frame":1,"at":70},{"type":"C","frame":0,"at":70}
      ]}]}"#;

    fn method<'a>(p: &'a Profile, name: &str) -> &'a MethodTimes {
        p.methods.iter().find(|m| p.frames[m.frame].method.starts_with(name)).unwrap_or_else(|| panic!("{name} in the profile"))
    }

    #[test]
    fn speedscope_stacks_become_a_call_tree_and_method_times() {
        let p = parse_speedscope(TRACE).unwrap();
        assert_eq!((p.threads, p.duration_ms), (1, 100.0));
        // 70 ms in all: 20 of them blocked (in Fast).
        assert_eq!(p.root().total, Times { cpu: 50.0, wall: 70.0 });
        // Slow: 10 own, 30 in Inner, then 10 more with itself on the stack twice (counted once).
        let slow = method(&p, "Work.Slow");
        assert_eq!(slow.own, Times { cpu: 20.0, wall: 20.0 });
        assert_eq!(slow.total, Times { cpu: 50.0, wall: 50.0 });
        assert_eq!(method(&p, "Work.Inner").own.cpu, 30.0);
        assert_eq!(method(&p, "Work.Fast").own, Times { cpu: 0.0, wall: 20.0 });
        assert_eq!(p.frames[p.methods[0].frame].method, "Work.Inner(int32)", "the most CPU time first");
        // The tree: Main → Slow → Inner, Main → Fast, Main → Slow → Slow.
        let main = p.children_sorted(0, true);
        assert_eq!(main.len(), 1, "the process and thread frames aren't in the tree");
        assert_eq!(p.frames[p.nodes[main[0]].frame].short(), "Program.<Main>$");
        let under_main = p.children_sorted(main[0], false);
        let names: Vec<String> = under_main.iter().map(|&n| p.frames[p.nodes[n].frame].short()).collect();
        assert_eq!(names, ["Work.Slow", "Work.Fast"]);
        assert_eq!(p.nodes[under_main[0]].total.wall, 50.0);
        assert_eq!(hot_path(&p, true, 0.05).len(), 3, "Main, Slow, Inner");
    }

    #[test]
    fn frame_names() {
        let f = Frame::parse("System.Private.CoreLib.il!System.Collections.Generic.List`1[System.__Canon].Add(!0)");
        assert_eq!(f.module, "System.Private.CoreLib");
        assert_eq!(f.short(), "List`1[System.__Canon].Add");
        assert_eq!(f.source_name(), Some(("List".into(), "Add".into())));
        let f = Frame::parse("Busy!Work.Slow(int32)");
        assert_eq!((f.module.as_str(), f.short().as_str()), ("Busy", "Work.Slow"));
        assert_eq!(f.source_name(), Some(("Work".into(), "Slow".into())));
        assert_eq!(Frame::parse("Busy!Order..ctor(int32,float64)").source_name(), None);
        assert_eq!(Frame::parse("App!App.Services.Orders.Load()").short(), "Orders.Load");
    }

    #[test]
    fn gcdump_report_rows_are_added_up_per_type() {
        let text = "      1,925,736  GC Heap bytes
         43,278  GC Heap objects

   Object Bytes     Count  Type
        524,312         1  Order[] (Bytes > 100K)  [Busy.dll]
          3,120         1  System.Int32[] (Bytes > 1K)  [System.Private.CoreLib.dll]
             32    43,056  Order  [Busy.dll]
             16         6  System.Int32[]  [System.Private.CoreLib.dll]
             32         1  System.Collections.Generic.List<Order>  [System.Private.CoreLib.dll]
";
        let r = parse_gcdump_report(text);
        assert_eq!((r.total_bytes, r.total_objects), (1_925_736, 43_278));
        assert_eq!(r.types[0], HeapType { name: "Order".into(), module: "Busy".into(), count: 43_056, bytes: 1_377_792 });
        assert_eq!(r.types[1].name, "Order[]");
        let ints = r.get("System.Int32[]").unwrap();
        assert_eq!((ints.count, ints.bytes), (7, 3_120 + 96));
        assert_eq!(r.get("System.Collections.Generic.List<Order>").unwrap().module, "System.Private.CoreLib");
        // Other locales group digits with dots or spaces.
        assert_eq!(parse_gcdump_report("  1.925.736  GC Heap bytes").total_bytes, 1_925_736);
    }
}
