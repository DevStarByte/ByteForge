//! The AI assistant side bar view: a chat with Claude (Anthropic API) or a local model (Ollama, LM Studio,
//! llama.cpp; "ai.provider"), which reads and searches the workspace through tools and proposes edits and
//! new files, each shown as a change to accept or reject. The loop runs here; see `bf_core::ai`.
//!
//! The conversation is quiet type in the side bar's own colors; a proposed change is the one thing drawn
//! like code: a diff in the editor's font and syntax colors, with the file's line numbers.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::VecDeque;

use bf_core::ai::{self, AiConfig, Part, Proposal, Provider, Role, Stop, StreamEvent, StreamHandle, ToolOutcome, Turn};
use bf_core::diff::RowKind;
use gpui::{Hsla, ScrollHandle, StyledText};

use super::*;

/// The file and place a message was about.
#[derive(Clone, Debug)]
struct Context_ {
    /// "Program.cs, line 12".
    label: String,
    path: PathBuf,
    line: usize,
}

/// One row of the chat as shown.
#[derive(Clone, Debug)]
enum Entry {
    User { text: String, context: Option<Context_> },
    Assistant { text: String, thinking: String, show_thinking: bool },
    /// A tool the assistant used: what it did, whether it failed.
    Tool { label: String, failed: bool },
    /// A change waiting for (or past) the user's decision; `line` is where it starts in the file (1-based).
    Change { tool_id: String, proposal: Proposal, line: usize, decided: Option<bool> },
    Notice { text: String, error: bool },
}

#[derive(Default)]
pub(super) struct AssistantState {
    input: Option<Entity<TextInput>>,
    _input_sub: Option<Subscription>,
    /// Whether the input has the keyboard (its frame shows it); set before each draw.
    pub(super) input_focused: bool,
    /// The conversation as sent to the model.
    turns: Vec<Turn>,
    entries: Vec<Entry>,
    /// An answer streaming in.
    request: Option<(StreamHandle, Task<()>)>,
    /// Running tool calls.
    tools_task: Option<Task<()>>,
    /// Results of the last answer's tool calls, collected while changes wait for the user.
    results: Vec<Part>,
    waiting: usize,
    /// Don't attach the active file and selection to messages.
    no_context: bool,
    scroll: ScrollHandle,
    tokens: (u64, u64),
    /// The local server's models, as last asked (for the settings): the server's URL and what it said.
    local_models: Option<(String, Result<Vec<String>, String>)>,
    local_models_task: Option<Task<()>>,
    /// Scripted answers instead of requests (tests).
    #[cfg(test)]
    script: VecDeque<Vec<StreamEvent>>,
    #[cfg(test)]
    sent: Vec<Vec<Turn>>,
}

impl AssistantState {
    pub(super) fn is_busy(&self) -> bool {
        self.request.is_some() || self.tools_task.is_some()
    }

    pub(super) fn input(&self) -> Option<&Entity<TextInput>> {
        self.input.as_ref()
    }

    /// The local server's models: None while not asked (or asking), else the list or why there's none.
    pub(super) fn local_models(&self, url: &str) -> Option<&Result<Vec<String>, String>> {
        self.local_models.as_ref().filter(|(u, _)| u == url).map(|(_, r)| r)
    }
}

/// What a tool call did, for the chat.
fn tool_label(name: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    match name {
        "read_file" => format!("Read {}", s("path")),
        "list_files" => format!("Listed {}", if s("path").is_empty() { "the workspace".to_owned() } else { s("path") }),
        "search" => format!("Searched for “{}”", s("query")),
        "edit_file" => format!("Proposed a change to {}", s("path")),
        "create_file" => format!("Proposed {}", s("path")),
        other => format!("Called {other}"),
    }
}

/// A theme color, else another key's (the assistant's keys fall back to the rest of the theme).
fn themed(t: &Theme, key: &str, fallback: &str) -> Hsla {
    if t.has(key) { t.c(key) } else { t.c(fallback) }
}

/// Where `old_text` starts in `text` (1-based line), 1 when it isn't there.
fn line_of(text: &str, old_text: &str) -> usize {
    text.replace("\r\n", "\n").find(old_text).map_or(1, |at| text.replace("\r\n", "\n")[..at].matches('\n').count() + 1)
}

impl Workbench {
    fn ai_config(&self) -> AiConfig {
        let key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()).or_else(|| self.settings.get_string("ai.apiKey"));
        AiConfig {
            provider: if self.settings.get_string("ai.provider").as_deref() == Some("local") { Provider::Local } else { Provider::Anthropic },
            api_key: key,
            model: self.settings.get_string("ai.model").unwrap_or_else(|| ai::DEFAULT_CLAUDE_MODEL.into()),
            effort: self.settings.get_string("ai.effort").unwrap_or_else(|| "high".into()),
            local_url: self.settings.get_string("ai.local.url").unwrap_or_else(|| ai::DEFAULT_LOCAL_URL.into()),
            local_model: self.settings.get_string("ai.local.model").unwrap_or_else(|| ai::DEFAULT_LOCAL_MODEL.into()),
            local_key: self.settings.get_string("ai.local.apiKey"),
        }
    }

    /// Asks the local server which models it has (once per server address, or again on Refresh).
    pub(super) fn refresh_local_models(&mut self, force: bool, cx: &mut Context<Self>) {
        let cfg = self.ai_config();
        let fresh = self.assistant.local_models.as_ref().is_some_and(|(u, _)| *u == cfg.local_url);
        if self.assistant.local_models_task.is_some() || (fresh && !force) {
            return;
        }
        if force {
            self.assistant.local_models = None;
        }
        let url = cfg.local_url.clone();
        self.assistant.local_models_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { ai::list_local_models(&cfg).map_err(|e| format!("{e:#}")) }).await;
            let _ = this.update(cx, |wb, cx| {
                wb.assistant.local_models_task = None;
                wb.assistant.local_models = Some((url, result));
                cx.notify();
            });
        }));
    }

    pub(super) fn ai_local_url(&self) -> String {
        self.settings.get_string("ai.local.url").unwrap_or_else(|| ai::DEFAULT_LOCAL_URL.into())
    }

    fn ensure_assistant_input(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = &self.assistant.input {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::multiline("Ask about the code, or ask for a change", 10, cx).bare());
        let sub = cx.subscribe(&input, |wb: &mut Self, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Confirmed) {
                let text = input.read(cx).text().to_owned();
                if !text.trim().is_empty() && !wb.assistant.is_busy() {
                    input.update(cx, |i, cx| i.set_text("", cx));
                    wb.send_to_assistant(text, cx);
                }
            }
        });
        self.assistant.input = Some(input.clone());
        self.assistant._input_sub = Some(sub);
        input
    }

    /// Shows the assistant and puts the caret in its input.
    pub(super) fn show_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_side(SideView::Assistant, cx);
        let input = self.ensure_assistant_input(cx);
        window.focus(&input.focus_handle(cx));
    }

    pub(super) fn new_assistant_chat(&mut self, cx: &mut Context<Self>) {
        self.stop_assistant(cx);
        let a = &mut self.assistant;
        a.turns.clear();
        a.entries.clear();
        a.results.clear();
        a.waiting = 0;
        a.tokens = (0, 0);
        cx.notify();
    }

    /// The active file and selection, as context for a message: what the chat shows, and what the model gets.
    fn assistant_context(&self, cx: &App) -> Option<(Context_, String)> {
        if self.assistant.no_context {
            return None;
        }
        let tab = self.active.and_then(|i| self.tabs.get(i)).filter(|t| t.is_document())?;
        let path = tab.path.clone()?;
        let rel = self.workspace.root().and_then(|r| path.strip_prefix(&r).ok().map(|p| p.to_string_lossy().replace('\\', "/"))).unwrap_or_else(|| path.display().to_string());
        let name = path.file_name().map_or_else(|| rel.clone(), |n| n.to_string_lossy().into_owned());
        let editor = tab.editor.read(cx);
        let caret = editor.state.caret();
        let (start, end) = editor.state.selection();
        if start != end {
            let text: String = editor.state.buffer.slice(start, end).chars().take(20_000).collect();
            let (a, b) = (start.line + 1, end.line + 1);
            let label = if a == b { format!("{name}, line {a} (selection)") } else { format!("{name}, lines {a}–{b}") };
            Some((
                Context_ { label, path, line: a },
                format!("<context>\nThe user is in {rel} and has selected lines {a}–{b}:\n```\n{text}\n```\n</context>\n\n"),
            ))
        } else {
            let line = caret.line + 1;
            Some((Context_ { label: format!("{name}, line {line}"), path, line }, format!("<context>\nThe user is in {rel}, at line {line}.\n</context>\n\n")))
        }
    }

    /// Sends a message. Changes still waiting for a decision count as rejected (the user moved on).
    pub(super) fn send_to_assistant(&mut self, text: String, cx: &mut Context<Self>) {
        let mut parts = self.reject_waiting_changes("The user moved on without deciding; the change wasn't made.");
        let context = self.assistant_context(cx);
        parts.push(Part::Text(match &context {
            Some((_, prompt)) => format!("{prompt}{text}"),
            None => text.clone(),
        }));
        self.assistant.entries.push(Entry::User { text, context: context.map(|c| c.0) });
        self.assistant.turns.push(Turn { role: Role::User, parts });
        self.request_answer(cx);
    }

    /// Every change still waiting: rejected, its tool result given (plus the results collected so far).
    fn reject_waiting_changes(&mut self, why: &str) -> Vec<Part> {
        let a = &mut self.assistant;
        for e in a.entries.iter_mut() {
            if let Entry::Change { tool_id, decided: decided @ None, .. } = e {
                *decided = Some(false);
                a.results.push(Part::ToolResult { id: tool_id.clone(), content: why.into(), is_error: false });
            }
        }
        a.waiting = 0;
        std::mem::take(&mut a.results)
    }

    fn request_answer(&mut self, cx: &mut Context<Self>) {
        let cfg = self.ai_config();
        let system = ai::system_prompt(self.workspace.root().as_deref(), std::env::consts::OS);
        self.assistant.entries.push(Entry::Assistant { text: String::new(), thinking: String::new(), show_thinking: false });
        self.assistant.scroll.scroll_to_bottom();
        #[cfg(test)]
        let started: anyhow::Result<(Option<StreamHandle>, flume::Receiver<StreamEvent>)> = {
            let _ = (&cfg, &system);
            self.assistant.sent.push(self.assistant.turns.clone());
            let (tx, rx) = flume::unbounded();
            for e in self.assistant.script.pop_front().unwrap_or_else(|| vec![StreamEvent::Error("nothing scripted".into())]) {
                let _ = tx.send(e);
            }
            Ok((None, rx))
        };
        #[cfg(not(test))]
        let started = ai::send(&cfg, &system, &self.assistant.turns).map(|(h, rx)| (Some(h), rx));
        let (handle, rx) = match started {
            Ok(s) => s,
            Err(e) => {
                self.assistant.entries.pop();
                self.assistant.entries.push(Entry::Notice { text: format!("{e:#}"), error: true });
                return cx.notify();
            }
        };
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv_async().await {
                let last = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error(_));
                if this.update(cx, |wb, cx| wb.on_assistant_event(event, cx)).is_err() || last {
                    break;
                }
            }
        });
        self.assistant.request = Some((handle.unwrap_or_else(StreamHandle::none), task));
        cx.notify();
    }

    fn current_answer(&mut self) -> Option<(&mut String, &mut String)> {
        match self.assistant.entries.last_mut() {
            Some(Entry::Assistant { text, thinking, .. }) => Some((text, thinking)),
            _ => None,
        }
    }

    fn on_assistant_event(&mut self, event: StreamEvent, cx: &mut Context<Self>) {
        match event {
            StreamEvent::Text(t) => {
                if let Some((text, _)) = self.current_answer() {
                    text.push_str(&t);
                }
            }
            StreamEvent::Thinking(t) => {
                if let Some((_, thinking)) = self.current_answer() {
                    thinking.push_str(&t);
                }
            }
            StreamEvent::ToolStarted(_) => {}
            StreamEvent::FellBack(model) => {
                self.assistant.entries.push(Entry::Notice { text: format!("{} took over after the model declined part of this.", ai::display_name(&model)), error: false })
            }
            StreamEvent::Error(e) => {
                self.assistant.request = None;
                self.drop_empty_answer();
                self.assistant.entries.push(Entry::Notice { text: e, error: true });
                // The message stays in the conversation; sending again retries with it.
                self.assistant.turns.push(Turn { role: Role::Assistant, parts: vec![Part::Text("(no answer: the request failed)".into())] });
            }
            StreamEvent::Done { turn, stop, input_tokens, output_tokens } => {
                self.assistant.request = None;
                self.assistant.tokens.0 += input_tokens;
                self.assistant.tokens.1 += output_tokens;
                self.drop_empty_answer();
                let calls = turn.tool_uses();
                self.assistant.turns.push(turn);
                match stop {
                    Stop::Refusal(category) => self.assistant.entries.push(Entry::Notice {
                        text: match category {
                            Some(c) => format!("The model declined to answer this ({c}). Rephrase the request, or start a new chat."),
                            None => "The model declined to answer this. Rephrase the request, or start a new chat.".into(),
                        },
                        error: true,
                    }),
                    Stop::MaxTokens => self.assistant.entries.push(Entry::Notice { text: "The answer hit the length limit and stops early. Ask it to continue.".into(), error: true }),
                    Stop::Other(why) => self.assistant.entries.push(Entry::Notice { text: format!("The answer stopped ({why})."), error: true }),
                    _ => {}
                }
                if !calls.is_empty() {
                    self.run_assistant_tools(calls, cx);
                }
            }
        }
        self.assistant.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn drop_empty_answer(&mut self) {
        if matches!(self.assistant.entries.last(), Some(Entry::Assistant { text, thinking, .. }) if text.is_empty() && thinking.is_empty()) {
            self.assistant.entries.pop();
        }
    }

    /// Runs the tool calls (off the UI thread): reading tools answer at once, changes wait for the user.
    fn run_assistant_tools(&mut self, calls: Vec<(String, String, serde_json::Value)>, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            let results = calls.into_iter().map(|(id, ..)| Part::ToolResult { id, content: "No folder is open in ByteForge.".into(), is_error: true }).collect();
            self.assistant.turns.push(Turn { role: Role::User, parts: results });
            return self.request_answer(cx);
        };
        // The open editors' text, unsaved changes included, wins over the disk.
        let open: HashMap<PathBuf, String> = self.tabs.iter().filter(|t| t.is_document()).filter_map(|t| Some((t.path.clone()?, t.editor.read(cx).state.buffer.text()))).collect();
        let task = cx.spawn(async move |this, cx| {
            let outcomes = cx
                .background_executor()
                .spawn(async move {
                    let read = |p: &Path| open.iter().find(|(o, _)| bf_core::paths::same_path(o, p)).map(|(_, t)| t.clone());
                    calls
                        .into_iter()
                        .map(|(id, name, input)| {
                            let label = tool_label(&name, &input);
                            let outcome = if input.get("__invalid_input").is_some() {
                                ToolOutcome::Done { content: "The tool input was cut off or isn't valid JSON; call the tool again.".into(), is_error: true }
                            } else {
                                ai::run_tool(&root, &name, &input, &read)
                            };
                            // Where a change starts, for the diff's line numbers.
                            let line = match &outcome {
                                ToolOutcome::Propose(Proposal::Edit { path, old_text, .. }) => {
                                    read(path).or_else(|| std::fs::read_to_string(path).ok()).map_or(1, |text| line_of(&text, old_text))
                                }
                                _ => 1,
                            };
                            (id, label, outcome, line)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                wb.assistant.tools_task = None;
                for (id, label, outcome, line) in outcomes {
                    match outcome {
                        ToolOutcome::Done { content, is_error } => {
                            wb.assistant.entries.push(Entry::Tool { label: if is_error { format!("{label}: {}", content.lines().next().unwrap_or("")) } else { label }, failed: is_error });
                            wb.assistant.results.push(Part::ToolResult { id, content, is_error });
                        }
                        ToolOutcome::Propose(proposal) => {
                            wb.assistant.waiting += 1;
                            wb.assistant.entries.push(Entry::Change { tool_id: id, proposal, line, decided: None });
                        }
                    }
                }
                wb.send_results_when_decided(cx);
                wb.assistant.scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        self.assistant.tools_task = Some(task);
    }

    /// All tool calls answered: the results go back in one message and the model goes on.
    fn send_results_when_decided(&mut self, cx: &mut Context<Self>) {
        if self.assistant.waiting > 0 || self.assistant.results.is_empty() {
            return;
        }
        let results = std::mem::take(&mut self.assistant.results);
        self.assistant.turns.push(Turn { role: Role::User, parts: results });
        self.request_answer(cx);
    }

    /// The user's yes or no to a proposed change (the entry at `ix`).
    fn decide_change(&mut self, ix: usize, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::Change { tool_id, proposal, decided, .. }) = self.assistant.entries.get(ix).cloned() else { return };
        if decided.is_some() {
            return;
        }
        let content = if accept {
            match self.apply_proposal(&proposal, window, cx) {
                Ok(where_) => format!("The user accepted the change; it's applied{where_}."),
                Err(e) => {
                    self.assistant.entries.push(Entry::Notice { text: format!("Couldn't apply the change: {e:#}"), error: true });
                    format!("The user accepted the change, but applying it failed: {e:#}")
                }
            }
        } else {
            "The user rejected this change; it wasn't made.".into()
        };
        if let Some(Entry::Change { decided, .. }) = self.assistant.entries.get_mut(ix) {
            *decided = Some(accept);
        }
        self.assistant.results.push(Part::ToolResult { id: tool_id, content, is_error: false });
        self.assistant.waiting = self.assistant.waiting.saturating_sub(1);
        self.send_results_when_decided(cx);
        cx.notify();
    }

    /// Makes an accepted change: in the open editor (unsaved, undoable) or on disk; a new file opens.
    fn apply_proposal(&mut self, proposal: &Proposal, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<&'static str> {
        match proposal {
            Proposal::Edit { path, old_text, new_text } => {
                let open = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))).map(|t| t.editor.clone());
                match open {
                    Some(editor) => {
                        let text = editor.read(cx).state.buffer.text();
                        anyhow::ensure!(text.matches(old_text.as_str()).count() == 1, "the text to replace changed in the editor meanwhile");
                        let new = text.replacen(old_text.as_str(), new_text, 1);
                        editor.update(cx, |e, cx| e.replace_text_minimal(&new, cx));
                        Ok(" in the open editor (not saved yet)")
                    }
                    None => {
                        ai::apply_edit_on_disk(path, old_text, new_text)?;
                        self.reload_changed_tabs(cx);
                        Ok(" and saved")
                    }
                }
            }
            Proposal::Create { path, content } => {
                ai::create_on_disk(path, content)?;
                self.open_file(path.clone(), None, window, cx);
                Ok(" (the file is created and open)")
            }
        }
    }

    /// Stops the answer streaming in (what came so far stays).
    pub(super) fn stop_assistant(&mut self, cx: &mut Context<Self>) {
        if let Some((handle, _task)) = self.assistant.request.take() {
            handle.cancel();
            let text = match self.assistant.entries.last() {
                Some(Entry::Assistant { text, .. }) => text.clone(),
                _ => String::new(),
            };
            self.drop_empty_answer();
            self.assistant.turns.push(Turn { role: Role::Assistant, parts: vec![Part::Text(if text.is_empty() { "(stopped)".into() } else { format!("{text}\n(stopped)") })] });
            self.assistant.entries.push(Entry::Notice { text: "Stopped.".into(), error: false });
        }
        self.assistant.tools_task = None;
        cx.notify();
    }

    /// Puts a starter question in the input, ready to send or edit.
    fn start_with(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.ensure_assistant_input(cx);
        input.update(cx, |i, cx| i.set_text(text, cx));
        window.focus(&input.focus_handle(cx));
    }

    // ───────────── drawing ─────────────

    pub(super) fn render_assistant(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let input = self.ensure_assistant_input(cx);
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let cfg = self.ai_config();
        let busy = self.assistant.is_busy();
        let speaker = match cfg.provider {
            Provider::Anthropic => "Claude".to_owned(),
            Provider::Local => cfg.local_model.clone(),
        };
        let user_rule = themed(&t, "assistant.userBorder", "focusBorder");
        let icon_button = |id: &'static str, name: &str, tip: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .p(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };

        // The model, quietly; how much this chat used is in its tooltip.
        let (tin, tout) = self.assistant.tokens;
        let model = match cfg.provider {
            Provider::Anthropic => ai::display_name(&cfg.model),
            Provider::Local => format!("{} on this machine", cfg.local_model),
        };
        let usage = if tin + tout > 0 { format!("This chat: {} tokens in, {} out", group_thousands(tin), group_thousands(tout)) } else { "Nothing sent yet".to_owned() };
        let header = div()
            .pl_3()
            .pr_2()
            .py(px(4.))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("assistant-model")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .text_color(muted)
                    .tooltip(tooltip(usage))
                    .child(model),
            )
            .child(icon_button("assistant-new", "add", "New chat".into()).on_click(cx.listener(|wb, _, _, cx| wb.new_assistant_chat(cx))));

        let mut list = div().id("assistant-chat").track_scroll(&self.assistant.scroll).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().px_3().pt_2().pb_3();
        if self.assistant.entries.is_empty() {
            list = list.child(self.render_assistant_empty(&cfg, cx));
        }
        let entries = self.assistant.entries.clone();
        let last = entries.len().saturating_sub(1);
        // A run of tool uses shows as one wrapping line; the speaker's name heads each of its turns.
        let mut tools: Vec<(String, bool)> = Vec::new();
        let mut after_user = true;
        let flush = |list: gpui::Stateful<gpui::Div>, tools: &mut Vec<(String, bool)>| {
            if tools.is_empty() {
                return list;
            }
            let row = div().mt(px(6.)).flex().flex_wrap().gap_x(px(12.)).gap_y(px(2.)).text_size(px(11.)).children(tools.drain(..).map(|(label, failed)| {
                let color = if failed { t.c("editorWarning.foreground") } else { muted };
                div().flex().items_center().gap(px(4.)).text_color(color).child(icon(if failed { "warning" } else { "check" }, px(11.), color)).child(label)
            }));
            list.child(row)
        };
        for (ix, entry) in entries.into_iter().enumerate() {
            if let Entry::Tool { label, failed } = entry {
                tools.push((label, failed));
                continue;
            }
            list = flush(list, &mut tools);
            match entry {
                Entry::User { text, context } => {
                    after_user = true;
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("assistant-user-{ix}")))
                            .mt(if ix == 0 { px(0.) } else { px(18.) })
                            .pl(px(10.))
                            .border_l_2()
                            .border_color(user_rule)
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().whitespace_normal().child(text))
                            .when_some(context, |d, c| {
                                let (path, line) = (c.path.clone(), c.line);
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("assistant-ctx-{ix}")))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .text_size(px(11.))
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(t.c("textLink.foreground")))
                                        .tooltip(tooltip("Open it there"))
                                        .child(icon("file", px(11.), muted))
                                        .child(c.label)
                                        .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(path.clone(), Some(Point::new(line.saturating_sub(1), 0)), w, cx))),
                                )
                            }),
                    );
                }
                Entry::Assistant { text, thinking, show_thinking } => {
                    let streaming = busy && ix == last;
                    let finished = !streaming && !text.is_empty();
                    let head = after_user.then(|| {
                        let copy = text.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).child(speaker.clone()))
                            .when(streaming, |d| d.child(div().text_size(px(12.)).text_color(muted).child(if thinking.is_empty() && text.is_empty() { "is working on it" } else { "is answering" })))
                            .child(div().flex_1())
                            .when(finished, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("assistant-copy-{ix}")))
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_size(px(11.))
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                                        .tooltip(tooltip("Copy the answer as Markdown"))
                                        .child("Copy")
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
                                )
                            })
                    });
                    after_user = false;
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("assistant-answer-{ix}")))
                            .debug_selector(move || format!("assistant-answer-{ix}"))
                            .mt(px(if head.is_some() { 14. } else { 6. }))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .children(head)
                            .when(!thinking.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.))
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("assistant-thinking-{ix}")))
                                                .flex()
                                                .items_center()
                                                .gap(px(2.))
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .cursor_pointer()
                                                .child(icon(if show_thinking { "chevronDown" } else { "chevronRight" }, px(11.), muted))
                                                .child(if show_thinking { "Hide reasoning" } else { "Show reasoning" })
                                                .on_click(cx.listener(move |wb, _, _, cx| {
                                                    if let Some(Entry::Assistant { show_thinking, .. }) = wb.assistant.entries.get_mut(ix) {
                                                        *show_thinking = !*show_thinking;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .when(show_thinking, |d| d.child(div().pl(px(13.)).text_size(px(12.)).text_color(muted).italic().whitespace_normal().child(thinking.clone()))),
                                )
                            })
                            .when(!text.is_empty(), |d| d.child(crate::markdown::render(&text, cx))),
                    );
                }
                Entry::Notice { text, error } => {
                    let color = if error { t.c("editorError.foreground") } else { muted };
                    list = list.child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(color)
                            .child(div().flex_none().pt(px(2.)).child(icon(if error { "error" } else { "info" }, px(12.), color)))
                            .child(div().flex_1().whitespace_normal().child(text)),
                    );
                }
                Entry::Change { proposal, line, decided, .. } => {
                    after_user = false;
                    list = list.child(div().mt(px(10.)).child(self.render_change(ix, &proposal, line, decided, cx)));
                }
                Entry::Tool { .. } => {}
            }
        }
        list = flush(list, &mut tools);

        div().size_full().flex().flex_col().text_size(px(13.)).child(header).child(list).child(self.render_composer(input, cx)).into_any_element()
    }

    /// Before the first message: what the assistant can do, and three ways to start.
    fn render_assistant_empty(&mut self, cfg: &AiConfig, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        if let Some(problem) = cfg.problem() {
            return div()
                .pt_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Connect a model"))
                .child(div().text_size(px(12.)).text_color(muted).whitespace_normal().child(problem))
                .into_any_element();
        }
        let about = match cfg.provider {
            Provider::Anthropic => "Claude reads and searches this workspace on its own. Changes it suggests show up here as diffs you accept or reject.".to_owned(),
            Provider::Local => format!(
                "{} answers from {}; nothing leaves this machine. It reads and searches the workspace, and its changes show up here as diffs to accept or reject.",
                cfg.local_model, cfg.local_url
            ),
        };
        let starters = ["Explain the open file", "Write tests for the selection", "Find bugs in this file"];
        div()
            .pt_2()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Ask about this workspace"))
            .child(div().text_size(px(12.)).text_color(muted).whitespace_normal().child(about))
            .child(div().flex().flex_col().gap(px(4.)).children(starters.into_iter().enumerate().map(|(i, s)| {
                div()
                    .id(SharedString::from(format!("assistant-starter-{i}")))
                    .debug_selector(move || format!("assistant-starter-{i}"))
                    .px_2()
                    .py(px(5.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(themed(&t, "assistant.starter.border", "input.border"))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|st| st.bg(t.c("list.hoverBackground")))
                    .child(s)
                    .on_click(cx.listener(move |wb, _, w, cx| wb.start_with(s, w, cx)))
            })))
            .into_any_element()
    }

    /// The input in a frame, with the file it will be about above it and send (or stop) in its corner.
    fn render_composer(&mut self, input: Entity<TextInput>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let busy = self.assistant.is_busy();
        let streaming = self.assistant.request.is_some();
        let context = self.assistant_context(cx).map(|c| c.0);
        let border = if self.assistant.input_focused { t.c("focusBorder") } else { themed(&t, "assistant.composer.border", "input.border") };
        let context_row = div().px_2().pt(px(6.)).flex().items_center().gap(px(4.)).text_size(px(11.)).text_color(muted).map(|d| match (&context, self.assistant.no_context) {
            (Some(c), false) => d.child(icon("file", px(11.), muted)).child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(c.label.clone())).child(
                div()
                    .id("assistant-context")
                    .debug_selector(|| "assistant-context".into())
                    .ml(px(2.))
                    .rounded(px(3.))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                    .tooltip(tooltip("Don't send the file along"))
                    .child(icon("close", px(11.), muted))
                    .on_click(cx.listener(|wb, _, _, cx| {
                        wb.assistant.no_context = true;
                        cx.notify();
                    })),
            ),
            (_, true) => d.child(
                div()
                    .id("assistant-context")
                    .debug_selector(|| "assistant-context".into())
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.c("textLink.foreground")))
                    .child(icon("add", px(11.), muted))
                    .child("Send the open file along")
                    .on_click(cx.listener(|wb, _, _, cx| {
                        wb.assistant.no_context = false;
                        cx.notify();
                    })),
            ),
            (None, false) => d.child("No file open"),
        });
        let action = if streaming {
            div()
                .id("assistant-stop")
                .debug_selector(|| "assistant-stop".into())
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .bg(t.c("button.secondaryBackground"))
                .tooltip(tooltip("Stop the answer"))
                .child(icon("stop", px(12.), t.c("icon.foreground")))
                .on_click(cx.listener(|wb, _, _, cx| wb.stop_assistant(cx)))
        } else {
            let empty = input.read(cx).text().trim().is_empty();
            let input = input.clone();
            div()
                .id("assistant-send")
                .debug_selector(|| "assistant-send".into())
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(t.c("button.background"))
                .when(empty || busy, |d| d.opacity(0.4))
                .when(!empty && !busy, |d| d.cursor_pointer())
                .tooltip(tooltip(format!("Send ({})", crate::actions::key_label("ctrl-enter"))))
                .child(icon("arrowUp", px(14.), t.c("button.foreground")))
                .on_click(cx.listener(move |wb, _, _, cx| {
                    let text = input.read(cx).text().to_owned();
                    if !text.trim().is_empty() && !wb.assistant.is_busy() {
                        input.update(cx, |i, cx| i.set_text("", cx));
                        wb.send_to_assistant(text, cx);
                    }
                }))
        };
        div()
            .p_2()
            .child(
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(border)
                    .bg(themed(&t, "assistant.composer.background", "input.background"))
                    .flex()
                    .flex_col()
                    .child(context_row)
                    .child(input)
                    .child(
                        div()
                            .px_2()
                            .pb(px(6.))
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_size(px(11.)).text_color(muted).child(format!("{} to send", crate::actions::key_label("ctrl-enter"))))
                            .child(action),
                    ),
            )
            .into_any_element()
    }

    /// A proposed change, drawn like code: the file, how many lines go and come, the diff in the editor's
    /// font and colors with the file's line numbers, then Accept and Reject (or what was decided).
    fn render_change(&mut self, ix: usize, proposal: &Proposal, line: usize, decided: Option<bool>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let root = self.workspace.root();
        let rel = |p: &Path| root.as_deref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let (path, title, old, new) = match proposal {
            Proposal::Edit { path, old_text, new_text } => (path.clone(), rel(path), old_text.as_str(), new_text.as_str()),
            Proposal::Create { path, content } => (path.clone(), format!("{} (new file)", rel(path)), "", content.as_str()),
        };
        let diff = bf_core::diff::compare(old, new);
        let pane = &diff.inline;
        let language = bf_syntax::Language::from_path(&path).map(|l| l.id());
        let fg = t.c("editor.foreground");
        let code_font = gpui::Font { weight: t.editor.font_weight, ..gpui::font(t.editor_font.clone()) };
        let size = px(t.editor.font_size.min(13.0) - 1.0);
        let numbers = themed(&t, "assistant.change.lineNumber", "editorLineNumber.foreground");
        let removed_bg = themed(&t, "assistant.change.removedBackground", "diffEditor.removedLineBackground");
        let added_bg = themed(&t, "assistant.change.addedBackground", "diffEditor.insertedLineBackground");
        let removed_fg = t.c_or("gitDecoration.deletedResourceForeground", 0xf85149ff);
        let added_fg = t.c_or("gitDecoration.addedResourceForeground", 0x2ea043ff);
        const MAX_ROWS: usize = 80;
        // The indentation every shown line has goes: in a narrow side bar the code starts at the left.
        let indent = pane.text.split('\n').filter(|l| !l.trim().is_empty()).map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count()).min().unwrap_or(0);
        let mut body = div().id(SharedString::from(format!("assistant-diff-{ix}"))).max_h(px(360.)).overflow_scroll().flex().flex_col().py(px(4.));
        for (row, text) in pane.text.split('\n').enumerate().take(MAX_ROWS) {
            let text: String = text.chars().skip(indent).collect();
            let text = text.as_str();
            let kind = pane.kinds.get(row).copied().unwrap_or(RowKind::Unchanged);
            let number = pane.numbers.get(row).copied().flatten().map(|n| (n + line - 1).to_string()).unwrap_or_default();
            let (sign, bg, sign_fg) = match kind {
                RowKind::Removed => ("−", Some(removed_bg), removed_fg),
                RowKind::Added => ("+", Some(added_bg), added_fg),
                _ => (" ", None, muted),
            };
            let runs = crate::markdown::code_runs(text, language, &code_font, fg, &t);
            body = body.child(
                div()
                    .flex()
                    .flex_none()
                    .min_w_full()
                    .pr_2()
                    .when_some(bg, |d, bg| d.bg(bg))
                    .font_family(t.editor_font.clone())
                    .text_size(size)
                    .whitespace_nowrap()
                    .child(div().w(px(36.)).flex_none().pr(px(6.)).flex().justify_end().text_color(numbers).child(number))
                    .child(div().w(px(14.)).flex_none().text_color(sign_fg).child(sign))
                    .child(div().flex_none().child(StyledText::new(SharedString::from(text.to_owned())).with_runs(runs))),
            );
        }
        if pane.kinds.len() > MAX_ROWS {
            body = body.child(div().pl(px(50.)).text_size(px(11.)).text_color(muted).child(format!("{} more lines; accept to see them in the file", pane.kinds.len() - MAX_ROWS)));
        }
        let button = |id: String, label: &'static str, primary: bool| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .px(px(10.))
                .py(px(3.))
                .rounded(px(4.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(primary, |d| d.bg(t.c("button.background")).text_color(t.c("button.foreground")))
                .when(!primary, |d| d.hover(|s| s.bg(t.c("toolbar.hoverBackground"))))
                .child(label)
        };
        let open_at = (path.clone(), line);
        div()
            .id(SharedString::from(format!("assistant-change-{ix}")))
            .rounded(px(6.))
            .border_1()
            .border_color(themed(&t, "assistant.change.border", "editorWidget.border"))
            .bg(themed(&t, "assistant.change.background", "editor.background"))
            .overflow_hidden()
            .when(decided == Some(false), |d| d.opacity(0.6))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(themed(&t, "assistant.change.border", "editorWidget.border"))
                    .text_size(px(12.))
                    .child(
                        div()
                            .id(SharedString::from(format!("assistant-change-file-{ix}")))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.c("textLink.foreground")))
                            .tooltip(tooltip("Open the file there"))
                            .child(title)
                            .on_click(cx.listener(move |wb, _, w, cx| {
                                let (p, l) = open_at.clone();
                                if p.exists() {
                                    wb.open_file(p, Some(Point::new(l.saturating_sub(1), 0)), w, cx);
                                }
                            })),
                    )
                    .when(diff.added > 0, |d| d.child(div().text_color(added_fg).child(format!("+{}", diff.added))))
                    .when(diff.removed > 0, |d| d.child(div().text_color(removed_fg).child(format!("−{}", diff.removed)))),
            )
            .child(body)
            .child(match decided {
                None => div()
                    .px_2()
                    .py(px(6.))
                    .flex()
                    .gap_1()
                    .border_t_1()
                    .border_color(themed(&t, "assistant.change.border", "editorWidget.border"))
                    .child(button(format!("assistant-accept-{ix}"), "Accept", true).on_click(cx.listener(move |wb, _, w, cx| wb.decide_change(ix, true, w, cx))))
                    .child(button(format!("assistant-reject-{ix}"), "Reject", false).on_click(cx.listener(move |wb, _, w, cx| wb.decide_change(ix, false, w, cx))))
                    .into_any_element(),
                Some(accepted) => div()
                    .px_2()
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .border_t_1()
                    .border_color(themed(&t, "assistant.change.border", "editorWidget.border"))
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(icon(if accepted { "check" } else { "close" }, px(11.), muted))
                    .child(if accepted { "Accepted" } else { "Rejected" })
                    .into_any_element(),
            })
            .into_any_element()
    }
}

/// 12345 → "12,345".
fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;
    use serde_json::json;

    fn done(parts: Vec<Part>, stop: Stop) -> StreamEvent {
        StreamEvent::Done { turn: Turn { role: Role::Assistant, parts }, stop, input_tokens: 1000, output_tokens: 200 }
    }

    #[test]
    fn names_and_lines() {
        assert_eq!(line_of("a\r\nb\r\nc", "c"), 3);
        assert_eq!(line_of("a\nb", "zz"), 1);
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    /// A conversation: the answer streams in; the model reads a file (with the active file as context), then
    /// proposes an edit, which waits for Accept and then lands in the open editor; the results go back and
    /// the model finishes. Reject sends "rejected"; a new message while a change waits rejects it.
    #[gpui::test]
    fn the_assistant_reads_proposes_and_applies_on_accept(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "assistant");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let file = ws.join("A.cs");
        std::fs::write(&file, "class A\n{\n    void Main() { }\n}\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
            wb.show_assistant(window, cx);
            let a = &mut wb.assistant;
            a.script.push_back(vec![
                StreamEvent::Thinking("Look first.".into()),
                StreamEvent::Text("Let me read it.".into()),
                done(vec![Part::Text("Let me read it.".into()), Part::ToolUse { id: "t1".into(), name: "read_file".into(), input: json!({"path": "A.cs"}) }], Stop::ToolUse),
            ]);
            a.script.push_back(vec![done(
                vec![Part::ToolUse { id: "t2".into(), name: "edit_file".into(), input: json!({"path": "A.cs", "old_text": "void Main() { }", "new_text": "void Run() { }"}) }],
                Stop::ToolUse,
            )]);
            a.script.push_back(vec![StreamEvent::Text("Done.".into()), done(vec![Part::Text("Done.".into())], Stop::EndTurn)]);
            wb.send_to_assistant("Rename Main to Run".into(), cx);
        });
        cx.run_until_parked();
        let (sent, entries) = wb.read_with(cx, |wb, _| (wb.assistant.sent.clone(), wb.assistant.entries.clone()));
        // The first message carries the active file as context.
        assert!(matches!(&sent[0][0].parts[0], Part::Text(t) if t.contains("The user is in A.cs") && t.ends_with("Rename Main to Run")));
        assert!(matches!(&entries[0], Entry::User { context: Some(c), .. } if c.label == "A.cs, line 1" && c.line == 1));
        // read_file ran at once and its result went back with the file's lines.
        assert!(matches!(&sent[1][2].parts[0], Part::ToolResult { id, content, is_error: false } if id == "t1" && content.contains("void Main()")));
        // The edit waits: no third request yet; its diff starts at the file's line 3.
        assert_eq!(sent.len(), 2);
        let change = entries.iter().position(|e| matches!(e, Entry::Change { decided: None, line: 3, .. })).expect("a change to decide, at line 3");
        assert!(entries.iter().any(|e| matches!(e, Entry::Assistant { thinking, .. } if thinking == "Look first.")));
        // Accept (clicked): it lands in the open editor, unsaved, and the model is told.
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear()
        });
        let accept: &'static str = Box::leak(format!("assistant-accept-{change}").into_boxed_str());
        let at = cx.debug_bounds(accept).expect("the Accept button").center();
        cx.simulate_click(at, gpui::Modifiers::none());
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            let tab = wb.tabs.iter().find(|t| t.path.as_deref() == Some(file.as_path())).unwrap();
            assert!(tab.editor.read(cx).state.buffer.text().contains("void Run() { }"));
            let sent = &wb.assistant.sent;
            assert_eq!(sent.len(), 3);
            assert!(matches!(&sent[2].last().unwrap().parts[0], Part::ToolResult { id, content, .. } if id == "t2" && content.contains("accepted")));
            assert!(matches!(wb.assistant.entries.last(), Some(Entry::Assistant { text, .. }) if text == "Done."));
            assert!(!wb.assistant.is_busy());
        });
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "class A\n{\n    void Main() { }\n}\n", "not saved yet");

        // A proposal left waiting is rejected by the next message, in the same message as it.
        wb.update(cx, |wb, cx| {
            wb.assistant.script.push_back(vec![done(
                vec![Part::ToolUse { id: "t3".into(), name: "create_file".into(), input: json!({"path": "B.cs", "content": "class B {}\n"}) }],
                Stop::ToolUse,
            )]);
            wb.assistant.script.push_back(vec![done(vec![Part::Text("OK.".into())], Stop::EndTurn)]);
            wb.assistant.no_context = true;
            wb.send_to_assistant("Add B".into(), cx);
        });
        cx.run_until_parked();
        wb.update(cx, |wb, cx| {
            assert_eq!(wb.assistant.waiting, 1);
            wb.send_to_assistant("Never mind".into(), cx);
        });
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| {
            let last = wb.assistant.sent.last().unwrap().last().unwrap();
            assert!(matches!(&last.parts[0], Part::ToolResult { id, content, .. } if id == "t3" && content.contains("moved on")));
            assert!(matches!(&last.parts[1], Part::Text(t) if t == "Never mind"), "no context when it's off");
        });
        assert!(!ws.join("B.cs").exists());

        // Errors and refusals show; New Chat clears.
        wb.update(cx, |wb, cx| {
            wb.assistant.script.push_back(vec![StreamEvent::Error("invalid x-api-key".into())]);
            wb.send_to_assistant("hi".into(), cx);
        });
        cx.run_until_parked();
        wb.update(cx, |wb, cx| {
            assert!(matches!(wb.assistant.entries.last(), Some(Entry::Notice { text, error: true }) if text == "invalid x-api-key"));
            wb.assistant.script.push_back(vec![done(Vec::new(), Stop::Refusal(Some("cyber".into())))]);
            wb.send_to_assistant("again".into(), cx);
        });
        cx.run_until_parked();
        wb.update(cx, |wb, cx| {
            assert!(matches!(wb.assistant.entries.last(), Some(Entry::Notice { text, .. }) if text.contains("declined")));
            wb.new_assistant_chat(cx);
            assert!(wb.assistant.entries.is_empty() && wb.assistant.turns.is_empty());
        });
        drop(guard);
    }

    /// Settings → AI Assistant offers models to pick: Claude's with the anthropic provider, the local
    /// server's with local; each provider's own settings only.
    #[gpui::test]
    fn the_settings_suggest_models_for_the_provider(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "assistant-models");
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear()
            });
        };
        let click = |cx: &mut gpui::VisualTestContext, id: &str| {
            draw(cx);
            let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
            let at = cx.debug_bounds(id).unwrap_or_else(|| panic!("{id} is drawn")).center();
            cx.simulate_click(at, gpui::Modifiers::none());
            cx.run_until_parked();
        };
        let listed = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| wb.read_with(cx, |wb, _| wb.listed_settings().into_iter().map(|s| s.key).filter(|k| k.starts_with("ai.")).collect::<Vec<_>>());
        wb.update_in(cx, |wb, window, cx| wb.open_settings_page(window, cx));
        // anthropic: the Claude models; picking Sonnet sets it.
        assert_eq!(listed(&wb, cx), ["ai.provider", "ai.model", "ai.effort", "ai.apiKey"]);
        let sonnet = ai::CLAUDE_MODELS.iter().position(|(id, _)| *id == "claude-sonnet-5-5").unwrap();
        // The settings screen lists more than fits: search narrows it to the AI section.
        wb.update(cx, |wb, cx| wb.settings_search.clone().unwrap().update(cx, |i, cx| i.set_text("ai.", cx)));
        click(cx, &format!("setting-model-ai.model-{sonnet}"));
        assert_eq!(wb.read_with(cx, |wb, _| wb.settings.get_string("ai.model")).as_deref(), Some("claude-sonnet-5-5"));

        // local: Claude's settings go, the server's models show (as the server listed them).
        wb.update(cx, |wb, cx| {
            wb.settings.set("ai.provider", json!("local"));
            let url = wb.ai_local_url();
            wb.assistant.local_models = Some((url, Ok(vec!["llama3.1:8b".into(), "qwen3.5:latest".into()])));
            cx.notify();
        });
        assert_eq!(listed(&wb, cx), ["ai.provider", "ai.local.url", "ai.local.model", "ai.local.apiKey"]);
        click(cx, "setting-model-ai.local.model-1");
        assert_eq!(wb.read_with(cx, |wb, _| wb.settings.get_string("ai.local.model")).as_deref(), Some("qwen3.5:latest"));
        // A server that doesn't answer: said so, with a way to ask again.
        wb.update(cx, |wb, cx| {
            let url = wb.ai_local_url();
            wb.assistant.local_models = Some((url, Err("Failed to connect".into())));
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("setting-models-refresh").is_some());
        drop(guard);
    }

    /// The empty chat offers starters; clicking one puts it in the input.
    #[gpui::test]
    fn a_starter_fills_the_input(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "assistant-empty");
        wb.update_in(cx, |wb, window, cx| {
            wb.settings.set("ai.provider", json!("local"));
            wb.show_assistant(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear()
        });
        let at = cx.debug_bounds("assistant-starter-0").expect("a starter").center();
        cx.simulate_click(at, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, cx| wb.assistant.input().unwrap().read(cx).text().to_owned()), "Explain the open file");
        drop(guard);
    }
}
