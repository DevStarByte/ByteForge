//! The AI assistant side bar view: a chat with Claude (Anthropic API) or a local model (Ollama, LM Studio,
//! llama.cpp; "ai.provider"), which reads and searches the workspace through tools and proposes edits and
//! new files, each shown as a change to accept or reject. The loop runs here; see `bf_core::ai`.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::VecDeque;

use bf_core::ai::{self, AiConfig, Part, Proposal, Provider, Role, Stop, StreamEvent, StreamHandle, ToolOutcome, Turn};
use gpui::ScrollHandle;

use super::*;

/// One row of the chat as shown.
#[derive(Clone, Debug)]
enum Entry {
    User { text: String, context: Option<String> },
    Assistant { text: String, thinking: String, show_thinking: bool },
    /// A tool the assistant used: what it did, whether it failed.
    Tool { label: String, failed: bool },
    /// A change waiting for (or past) the user's decision.
    Change { tool_id: String, proposal: Proposal, decided: Option<bool> },
    Notice { text: String, error: bool },
}

#[derive(Default)]
pub(super) struct AssistantState {
    input: Option<Entity<TextInput>>,
    _input_sub: Option<Subscription>,
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
    /// Attach the active file and selection to each message.
    no_context: bool,
    scroll: ScrollHandle,
    tokens: (u64, u64),
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
}

/// What a tool call did, for the chat.
fn tool_label(name: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    match name {
        "read_file" => format!("Read {}", s("path")),
        "list_files" => format!("Listed {}", if s("path").is_empty() { "the workspace".to_owned() } else { s("path") }),
        "search" => format!("Searched for “{}”", s("query")),
        "edit_file" => format!("Proposed a change to {}", s("path")),
        "create_file" => format!("Proposed a new file {}", s("path")),
        other => format!("Called {other}"),
    }
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

    fn ensure_assistant_input(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = &self.assistant.input {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::multiline(format!("Ask about the code or ask for a change ({} sends)", crate::actions::key_label("ctrl-enter")), 8, cx));
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

    /// The active file and selection, as context for a message: (what the chat shows, what the model gets).
    fn assistant_context(&self, cx: &App) -> Option<(String, String)> {
        if self.assistant.no_context {
            return None;
        }
        let tab = self.active.and_then(|i| self.tabs.get(i)).filter(|t| t.is_document())?;
        let path = tab.path.clone()?;
        let rel = self.workspace.root().and_then(|r| path.strip_prefix(&r).ok().map(|p| p.to_string_lossy().replace('\\', "/"))).unwrap_or_else(|| path.display().to_string());
        let editor = tab.editor.read(cx);
        let caret = editor.state.caret();
        let (start, end) = editor.state.selection();
        if start != end {
            let text = editor.state.buffer.slice(start, end);
            let text: String = text.chars().take(20_000).collect();
            Some((
                format!("{rel}, lines {}–{}", start.line + 1, end.line + 1),
                format!("<context>\nThe user is in {rel} and has selected lines {}–{}:\n```\n{text}\n```\n</context>\n\n", start.line + 1, end.line + 1),
            ))
        } else {
            Some((format!("{rel}, line {}", caret.line + 1), format!("<context>\nThe user is in {rel}, at line {}.\n</context>\n\n", caret.line + 1)))
        }
    }

    /// Sends a message. Changes still waiting for a decision count as rejected (the user moved on).
    pub(super) fn send_to_assistant(&mut self, text: String, cx: &mut Context<Self>) {
        let mut parts = self.reject_waiting_changes("The user moved on without deciding; the change wasn't made.");
        let context = self.assistant_context(cx);
        parts.push(Part::Text(match &context {
            Some((_, ctx)) => format!("{ctx}{text}"),
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
            StreamEvent::FellBack(model) => self.assistant.entries.push(Entry::Notice { text: format!("The model declined part of this; {model} took over."), error: false }),
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
                            Some(c) => format!("The model declined to answer this ({c})."),
                            None => "The model declined to answer this.".into(),
                        },
                        error: true,
                    }),
                    Stop::MaxTokens => self.assistant.entries.push(Entry::Notice { text: "The answer hit the length limit and was cut off.".into(), error: true }),
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
                            (id, label, outcome)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                wb.assistant.tools_task = None;
                for (id, label, outcome) in outcomes {
                    match outcome {
                        ToolOutcome::Done { content, is_error } => {
                            wb.assistant.entries.push(Entry::Tool { label: if is_error { format!("{label}: {}", content.lines().next().unwrap_or("")) } else { label }, failed: is_error });
                            wb.assistant.results.push(Part::ToolResult { id, content, is_error });
                        }
                        ToolOutcome::Propose(proposal) => {
                            wb.assistant.waiting += 1;
                            wb.assistant.entries.push(Entry::Change { tool_id: id, proposal, decided: None });
                        }
                    }
                }
                wb.send_results_when_decided(cx);
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
        let Some(Entry::Change { tool_id, proposal, decided }) = self.assistant.entries.get(ix).cloned() else { return };
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

    // ───────────── drawing ─────────────

    pub(super) fn render_assistant(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let input = self.ensure_assistant_input(cx);
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let cfg = self.ai_config();
        let busy = self.assistant.is_busy();
        let small = |id: &'static str, name: &str, tip: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .p(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };
        let header = div()
            .px_3()
            .py_1()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(11.))
            .text_color(muted)
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(cfg.model_label()))
            .when(self.assistant.tokens.0 + self.assistant.tokens.1 > 0, |d| {
                d.child(div().flex_none().child(format!("{}k in · {}k out", self.assistant.tokens.0 / 1000, self.assistant.tokens.1 / 1000)))
            })
            .child(small("assistant-new", "add", "New Chat".into()).on_click(cx.listener(|wb, _, _, cx| wb.new_assistant_chat(cx))));

        let mut list = div().id("assistant-chat").track_scroll(&self.assistant.scroll).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_2().px_3().py_2();
        if self.assistant.entries.is_empty() {
            let hint = match cfg.problem() {
                Some(p) => p,
                None => match cfg.provider {
                    Provider::Anthropic => "Ask about this code, or for a change. Claude reads and searches the workspace itself; every change it proposes shows up here to accept or reject.".into(),
                    Provider::Local => format!("Talking to {} on {}. Tool use needs a model that supports it (Qwen 3.5, Llama 3.1 and later).", cfg.local_model, cfg.local_url),
                },
            };
            list = list.child(div().text_size(px(12.)).text_color(muted).child(hint));
        }
        let entries = self.assistant.entries.clone();
        let last = entries.len().saturating_sub(1);
        for (ix, entry) in entries.into_iter().enumerate() {
            let el: gpui::AnyElement = match entry {
                Entry::User { text, context } => div()
                    .id(SharedString::from(format!("assistant-user-{ix}")))
                    .p_2()
                    .rounded(px(4.))
                    .bg(t.c("input.background"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when_some(context, |d, c| d.child(div().text_size(px(11.)).text_color(muted).child(c)))
                    .child(div().whitespace_normal().child(text))
                    .into_any_element(),
                Entry::Assistant { text, thinking, show_thinking } => {
                    let streaming = busy && ix == last;
                    div()
                        .id(SharedString::from(format!("assistant-answer-{ix}")))
                        .debug_selector(move || format!("assistant-answer-{ix}"))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(!thinking.is_empty(), |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("assistant-thinking-{ix}")))
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .cursor_pointer()
                                    .child(if show_thinking { format!("▾ Thinking\n{thinking}") } else { "▸ Thinking".into() })
                                    .on_click(cx.listener(move |wb, _, _, cx| {
                                        if let Some(Entry::Assistant { show_thinking, .. }) = wb.assistant.entries.get_mut(ix) {
                                            *show_thinking = !*show_thinking;
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(!text.is_empty(), |d| d.child(crate::markdown::render(&text, cx)))
                        .when(text.is_empty() && thinking.is_empty() && streaming, |d| d.child(div().text_color(muted).child("…")))
                        .when(!text.is_empty() && !streaming, |d| {
                            let copy = text.clone();
                            d.child(
                                div().flex().child(
                                    div()
                                        .id(SharedString::from(format!("assistant-copy-{ix}")))
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_size(px(11.))
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                                        .child("Copy")
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
                                ),
                            )
                        })
                        .into_any_element()
                }
                Entry::Tool { label, failed } => div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(if failed { t.c("editorWarning.foreground") } else { muted })
                    .child(icon(if failed { "warning" } else { "check" }, px(12.), if failed { t.c("editorWarning.foreground") } else { muted }))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
                    .into_any_element(),
                Entry::Notice { text, error } => {
                    div().text_size(px(12.)).text_color(if error { t.c("editorError.foreground") } else { muted }).whitespace_normal().child(text).into_any_element()
                }
                Entry::Change { proposal, decided, .. } => self.render_change(ix, &proposal, decided, cx),
            };
            list = list.child(el);
        }

        let send_or_stop = if self.assistant.request.is_some() {
            div()
                .id("assistant-stop")
                .debug_selector(|| "assistant-stop".into())
                .px_2()
                .py(px(3.))
                .rounded(px(3.))
                .cursor_pointer()
                .bg(t.c("button.secondaryBackground"))
                .child("Stop")
                .on_click(cx.listener(|wb, _, _, cx| wb.stop_assistant(cx)))
        } else {
            let input = input.clone();
            div()
                .id("assistant-send")
                .debug_selector(|| "assistant-send".into())
                .px_2()
                .py(px(3.))
                .rounded(px(3.))
                .cursor_pointer()
                .bg(t.c("button.background"))
                .text_color(t.c("button.foreground"))
                .when(busy, |d| d.opacity(0.5))
                .child("Send")
                .on_click(cx.listener(move |wb, _, _, cx| {
                    let text = input.read(cx).text().to_owned();
                    if !text.trim().is_empty() && !wb.assistant.is_busy() {
                        input.update(cx, |i, cx| i.set_text("", cx));
                        wb.send_to_assistant(text, cx);
                    }
                }))
        };
        let context = self.assistant_context(cx).map(|c| c.0);
        let footer = div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(t.c("panel.border"))
            .child(div().rounded(px(3.)).border_1().border_color(t.c("input.border")).child(input))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(
                        div()
                            .id("assistant-context")
                            .debug_selector(|| "assistant-context".into())
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .tooltip(tooltip("Click: send the active file and selection along with messages, or not"))
                            .child(match (&context, self.assistant.no_context) {
                                (_, true) => "No file context".to_owned(),
                                (Some(c), _) => format!("With {c}"),
                                (None, _) => "No file open".to_owned(),
                            })
                            .on_click(cx.listener(|wb, _, _, cx| {
                                wb.assistant.no_context = !wb.assistant.no_context;
                                cx.notify();
                            })),
                    )
                    .child(send_or_stop),
            );
        div().size_full().flex().flex_col().text_size(px(13.)).child(header).child(list).child(footer).into_any_element()
    }

    /// A proposed change: the file, the lines that go and come, Accept and Reject (or what was decided).
    fn render_change(&mut self, ix: usize, proposal: &Proposal, decided: Option<bool>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let root = self.workspace.root();
        let rel = |p: &Path| root.as_deref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let (title, removed, added) = match proposal {
            Proposal::Edit { path, old_text, new_text } => (format!("Change {}", rel(path)), old_text.clone(), new_text.clone()),
            Proposal::Create { path, content } => (format!("New file {}", rel(path)), String::new(), content.clone()),
        };
        let line = |text: String, sign: &str, bg: gpui::Hsla| {
            div().px_1().bg(bg).font_family(t.editor_font.clone()).text_size(px(12.)).whitespace_nowrap().overflow_hidden().child(format!("{sign} {text}"))
        };
        let removed_bg = t.c_or("diffEditor.removedLineBackground", 0xff000033);
        let added_bg = t.c_or("diffEditor.insertedLineBackground", 0x00ff0033);
        let mut diff = div().flex().flex_col().max_h(px(320.)).id(SharedString::from(format!("assistant-diff-{ix}"))).overflow_y_scroll();
        const MAX: usize = 60;
        for l in removed.lines().take(MAX) {
            diff = diff.child(line(l.to_owned(), "-", removed_bg));
        }
        for l in added.lines().take(MAX) {
            diff = diff.child(line(l.to_owned(), "+", added_bg));
        }
        if removed.lines().count() > MAX || added.lines().count() > MAX {
            diff = diff.child(div().text_size(px(11.)).text_color(muted).child("…"));
        }
        let button = |id: String, label: &'static str, primary: bool| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .px_2()
                .py(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .when(primary, |d| d.bg(t.c("button.background")).text_color(t.c("button.foreground")))
                .when(!primary, |d| d.bg(t.c("button.secondaryBackground")))
                .child(label)
        };
        div()
            .id(SharedString::from(format!("assistant-change-{ix}")))
            .rounded(px(4.))
            .border_1()
            .border_color(t.c("panel.border"))
            .flex()
            .flex_col()
            .child(div().px_2().py_1().text_size(px(12.)).child(title))
            .child(diff)
            .child(match decided {
                Some(true) => div().px_2().py_1().text_size(px(11.)).text_color(muted).child("Accepted").into_any_element(),
                Some(false) => div().px_2().py_1().text_size(px(11.)).text_color(muted).child("Rejected").into_any_element(),
                None => div()
                    .px_2()
                    .py_1()
                    .flex()
                    .gap_2()
                    .child(button(format!("assistant-accept-{ix}"), "Accept", true).on_click(cx.listener(move |wb, _, w, cx| wb.decide_change(ix, true, w, cx))))
                    .child(button(format!("assistant-reject-{ix}"), "Reject", false).on_click(cx.listener(move |wb, _, w, cx| wb.decide_change(ix, false, w, cx))))
                    .into_any_element(),
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;
    use serde_json::json;

    fn done(parts: Vec<Part>, stop: Stop) -> StreamEvent {
        StreamEvent::Done { turn: Turn { role: Role::Assistant, parts }, stop, input_tokens: 1000, output_tokens: 200 }
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
        // read_file ran at once and its result went back with the file's lines.
        assert!(matches!(&sent[1][2].parts[0], Part::ToolResult { id, content, is_error: false } if id == "t1" && content.contains("void Main()")));
        // The edit waits: no third request yet.
        assert_eq!(sent.len(), 2);
        let change = entries.iter().position(|e| matches!(e, Entry::Change { decided: None, .. })).expect("a change to decide");
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
}
