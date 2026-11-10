//! The workbench: window layout, tabs, panels, title bar, menus, status bar and all workbench commands.

mod debug;
mod module_views;
mod modules;
mod preview;
mod testing;
mod references;
mod solution;
mod symbols;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bf_core::dotnet::{DotnetProcess, RunEvent, find_target};
use bf_core::plugins::{ModuleManifest, PluginHandle, PluginMessage, discover};
use bf_core::settings::State;
use bf_core::{AppPaths, Settings, ThemeService, Workspace, log_line};
use bf_lsp::{LspClient, LspEvent, ServerConfig};
use bf_text::Point;
use gpui::{
    AnyView, App, AppContext, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathPromptOptions, PromptLevel, SharedString, Subscription, Task, Window, WindowBackgroundAppearance, WindowControlArea, div,
    prelude::*, px,
};
use serde_json::json;

use crate::actions::*;
use crate::assets::{self, icon};
use crate::solution::SolutionView;
use crate::testing::TestView;
use crate::editor::{EditorEvent, EditorView};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::text_input::{InputEvent, TextInput};
use crate::explorer::{Explorer, ExplorerEvent};
use crate::palette::{Mode, Palette, PaletteAction, PaletteEvent, PaletteItem};
use crate::scm::{ScmEvent, ScmView};
use crate::search::{SearchEvent, SearchView};
use crate::theme::Theme;

pub enum AppEvent {
    ThemeFilesChanged,
    SettingsChanged,
    FilesChanged,
}

struct Tab {
    editor: Entity<EditorView>,
    /// Editor group: 0 = left, 1 = right (split editor).
    group: usize,
    path: Option<PathBuf>,
    title: String,
    bom: bool,
    /// A page instead of a document. The tab keeps an empty, read-only editor so editor-wide code keeps
    /// working; the editor area draws the page instead.
    page: Option<Page>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, PartialEq)]
enum Page {
    /// The details of the module with this id.
    Module(String),
    /// A live preview of this Markdown file.
    Markdown(PathBuf),
}

impl Tab {
    fn module_page(&self) -> Option<&str> {
        match &self.page {
            Some(Page::Module(id)) => Some(id),
            _ => None,
        }
    }

    fn markdown_page(&self) -> Option<&Path> {
        match &self.page {
            Some(Page::Markdown(path)) => Some(path),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideView {
    Explorer,
    Search,
    SourceControl,
    Debug,
    Testing,
    Solution,
    /// A module's view (which one: `module_view`).
    Module,
    Modules,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelView {
    Problems,
    Output,
    DebugConsole,
    Terminal,
    References,
}

#[derive(Clone, Copy)]
enum Sash {
    Side { start_x: f32, start_w: f32, right: bool },
    /// `top`: the panel sits above the editor, so dragging down makes it taller.
    Panel { start_y: f32, start_h: f32, top: bool },
    PanelSide { start_x: f32, start_w: f32, right: bool },
}

/// Where the panel is placed (from the theme layout).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelPlace {
    Top,
    Bottom,
    Left,
    Right,
}

enum PromptKind {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    CustomizeTheme,
    CreateBranch,
    RenameSymbol { editor: gpui::WeakEntity<EditorView>, at: Point, old: String },
}

/// Runs when a dotnet process started by `start_dotnet` exits (argument: whether it succeeded).
type DotnetDone = Box<dyn FnOnce(&mut Workbench, bool, &mut Window, &mut Context<Workbench>)>;
/// Called with each output line of a dotnet process.
type LineHook = Box<dyn FnMut(&mut Workbench, &str, &mut Window, &mut Context<Workbench>)>;

/// A formatter contributed by a module.
struct Formatter {
    module: String,
    id: String,
    name: String,
    extensions: Vec<String>,
}

/// A format request waiting for its module's answer.
struct PendingFormat {
    editor: gpui::WeakEntity<EditorView>,
    /// The buffer version the text was sent at; a result for an older text is dropped.
    version: u64,
    save_after: bool,
    name: String,
}

struct StatusEntry {
    module: String,
    id: String,
    text: String,
    tooltip: Option<String>,
    command: Option<String>,
    right: bool,
}

const MENUS: &[(&str, &[&str])] = &[
    ("File", &["file.newFile", "file.openFile", "file.openFolder", "-", "file.save", "file.saveAs", "file.saveAll", "-", "file.close", "file.closeAll", "-", "app.exit"]),
    ("Edit", &["edit.undo", "edit.redo", "-", "edit.find", "edit.replace", "-", "edit.toggleComment", "edit.duplicateLines", "edit.triggerSuggest", "edit.signatureHelp", "-", "edit.rename", "-", "edit.toggleLineEnding"]),
    ("View", &["view.commandPalette", "view.quickOpen", "-", "view.explorer", "view.search", "view.scm", "view.debug", "view.testing", "view.modules", "view.problems", "view.output", "view.debugConsole", "-", "terminal.toggle", "terminal.new", "terminal.selectDefaultShell", "-", "view.toggleSideBar", "view.togglePanel"]),
    ("Go", &["view.quickOpen", "edit.goToLine", "edit.goToDefinition", "edit.goToImplementation", "edit.goToTypeDefinition", "edit.findReferences", "-", "view.goToSymbol", "view.goToWorkspaceSymbol", "-", "view.nextEditor", "view.previousEditor"]),
    ("Git", &["git.commit", "git.refresh", "-", "git.pull", "git.push", "git.sync", "git.fetch", "-", "git.checkout", "git.createBranch", "-", "git.stageAll", "git.unstageAll", "git.openChanges", "-", "git.init"]),
    ("Run", &[
        "debug.start", "dotnet.run", "debug.restart", "dotnet.stop", "-", "debug.stepOver", "debug.stepInto", "debug.stepOut", "debug.pause", "-",
        "debug.toggleBreakpoint", "debug.removeAllBreakpoints", "debug.attach", "-", "dotnet.build", "dotnet.test", "dotnet.clean", "dotnet.restore", "-",
        "test.runAll", "test.rerunFailed", "test.runAtCursor", "test.debugAtCursor", "-",
        "csharp.reload",
    ]),
    ("Preferences", &["settings.open", "-", "theme.select", "theme.customize", "theme.edit", "theme.reload"]),
    ("Help", &["view.modules", "app.about"]),
];

pub struct Workbench {
    paths: AppPaths,
    settings: Settings,
    state: State,
    themes: ThemeService,
    workspace: Workspace,
    tabs: Vec<Tab>,
    /// The focused tab (the visible one of the focused group).
    active: Option<usize>,
    /// The tab shown in each editor group, and which group has focus.
    visible: [Option<usize>; 2],
    focus_group: usize,
    untitled: usize,
    explorer: Entity<Explorer>,
    search: Entity<SearchView>,
    scm: Entity<ScmView>,
    tests: Entity<TestView>,
    solution: Entity<SolutionView>,
    /// Numbers the results folders of test runs.
    test_runs: u64,
    git_head: Option<Option<String>>,
    output: Entity<EditorView>,
    palette: Option<(Entity<Palette>, Subscription)>,
    palette_restore_theme: Option<String>,
    /// Loads symbols for the palette ("@" and "#"); replaced by each new query.
    symbol_task: Option<Task<()>>,
    prompt: Option<PromptKind>,
    side: SideView,
    side_visible: bool,
    side_width: f32,
    panel: PanelView,
    /// The last Find All References result (the References panel).
    references: Option<references::References>,
    panel_visible: bool,
    panel_height: f32,
    panel_width: f32,
    sash: Option<Sash>,
    menu_open: Option<usize>,
    lsp: Option<LspClient>,
    lsp_status: String,
    lsp_task: Option<Task<()>>,
    dotnet: Option<DotnetProcess>,
    dotnet_task: Option<Task<()>>,
    /// Compiler errors and warnings from the last build (Razor files only get diagnostics this way).
    build_problems: Vec<(PathBuf, crate::editor::Diagnostic)>,
    debug: Option<debug::DebugSession>,
    debug_generation: u64,
    /// Breakpoints of all files (open or not), sorted by line.
    breakpoints: BTreeMap<PathBuf, Vec<crate::editor::Breakpoint>>,
    debug_console: Entity<EditorView>,
    debug_input: Entity<TextInput>,
    terminals: Vec<(Entity<TerminalView>, Subscription)>,
    active_terminal: usize,
    plugins: Vec<PluginHandle>,
    /// Every module found, running or removed (for the Modules view).
    modules: Vec<ModuleManifest>,
    /// The module registry (Modules view, "AVAILABLE").
    registry: modules::RegistryState,
    /// The module being installed, updated or uninstalled.
    module_busy: Option<String>,
    module_filter: Entity<TextInput>,
    /// The module whose details are open in the Modules view.
    registry_task: Option<Task<()>>,
    md_cache: preview::DiskCache,
    plugin_tx: Option<flume::Sender<PluginMessage>>,
    formatters: Vec<Formatter>,
    format_requests: HashMap<u64, PendingFormat>,
    next_format_request: u64,
    plugin_commands: HashMap<String, String>,
    plugin_status: Vec<StatusEntry>,
    /// Side bar views of the turned-on modules, the one shown (with `SideView::Module`), and the one the
    /// module was last told is visible.
    module_views: Vec<module_views::ModuleView>,
    /// The open context menu of a module view's row.
    module_menu: Option<module_views::ModuleMenu>,
    module_view: Option<String>,
    module_view_shown: Option<(String, String)>,
    output_tx: flume::Sender<String>,
    focus: FocusHandle,
    theme_version: usize,
    force_close: bool,
    custom_titlebar: bool,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        div()
            .bg(t.bg("editorHoverWidget.background"))
            .text_color(t.c("editorHoverWidget.foreground"))
            .border_1()
            .border_color(t.c("editorHoverWidget.border"))
            .rounded(px(4.))
            .px_2()
            .py_1()
            .text_size(px(12.))
            .font_family(t.ui_font.clone())
            .child(self.0.clone())
    }
}

/// Canonical long path without the `\\?\` prefix (language servers match documents by exact path).
pub fn normalize_path(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => canonical,
    }
}

impl Workbench {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        paths: AppPaths,
        settings: Settings,
        themes: ThemeService,
        events: flume::Receiver<AppEvent>,
        event_tx: flume::Sender<AppEvent>,
        args: Vec<String>,
        custom_titlebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let excludes: Vec<String> = settings.get("files.exclude", vec!["bin".into(), "obj".into(), ".git".into(), "node_modules".into(), "target".into()]);
        let workspace = Workspace::new(excludes);
        let explorer = cx.new(|cx| Explorer::new(workspace.clone(), cx));
        let search = cx.new(|cx| SearchView::new(workspace.clone(), window, cx));
        let scm = cx.new(|cx| ScmView::new(workspace.clone(), window, cx));
        let tests = cx.new(|cx| TestView::new(workspace.clone(), cx));
        let solution = cx.new(|cx| {
            let mut v = SolutionView::new(workspace.clone(), cx);
            v.set_preferred(settings.get_string("dotnet.project"));
            v
        });
        let output = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e.show_line_numbers = false;
            e.show_indent_guides = false;
            e.detect_links = true;
            e
        });
        let debug_console = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e.show_line_numbers = false;
            e.show_indent_guides = false;
            e.detect_links = true;
            e
        });
        let debug_input = cx.new(|cx| TextInput::new("Evaluate an expression while paused (Enter)", cx));
        let module_filter = cx.new(|cx| TextInput::new("Search modules", cx));
        let (output_tx, output_rx) = flume::unbounded::<String>();
        let state = State::load(paths.state_file());
        let theme_version = Theme::global(cx).version;

        let mut subs = vec![
            cx.subscribe_in(&explorer, window, Self::on_explorer_event),
            cx.subscribe_in(&search, window, |wb, _, event, window, cx| match event {
                SearchEvent::Open(path, point) => wb.open_file(path.clone(), Some(*point), window, cx),
            }),
            cx.subscribe_in(&scm, window, Self::on_scm_event),
            cx.subscribe_in(&tests, window, Self::on_test_event),
            cx.subscribe_in(&solution, window, Self::on_solution_event),
            // File references in program output open on click.
            cx.subscribe_in(&output, window, Self::on_output_event),
            cx.subscribe_in(&debug_console, window, Self::on_output_event),
            cx.subscribe_in(&module_filter, window, |_, _, event, _, cx| {
                if let InputEvent::Changed = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&debug_input, window, |wb, input, event, _, cx| {
                if let InputEvent::Confirmed = event {
                    let expression = input.read(cx).text().trim().to_owned();
                    if !expression.is_empty() {
                        input.update(cx, |i, cx| i.set_text("", cx));
                        wb.debug_evaluate(expression, cx);
                    }
                }
            }),
        ];
        subs.push(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));

        let mut tasks = Vec::new();
        // Batched output pump.
        tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(first) = output_rx.recv_async().await {
                let mut text = first;
                text.push('\n');
                while let Ok(more) = output_rx.try_recv() {
                    text.push_str(&more);
                    text.push('\n');
                }
                if this.update(cx, |wb, cx| wb.output.update(cx, |o, cx| o.append(&text, cx))).is_err() {
                    break;
                }
            }
        }));
        // File system / settings / theme notifications from background watchers.
        tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let ok = this
                    .update_in(cx, |wb, window, cx| match event {
                        AppEvent::ThemeFilesChanged => wb.reload_theme(window, cx),
                        AppEvent::SettingsChanged => wb.apply_settings(window, cx),
                        AppEvent::FilesChanged => {
                            wb.explorer.update(cx, |e, cx| e.refresh(cx));
                            for p in &wb.plugins {
                                p.event("filesChanged", None);
                            }
                            if wb.tests.read(cx).is_discovered() {
                                wb.tests.update(cx, |t, cx| t.files_changed(cx));
                            }
                            if wb.solution.read(cx).is_loaded() {
                                wb.solution.update(cx, |s, cx| s.load(cx));
                            }
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        }));

        let mut wb = Self {
            paths,
            settings,
            state,
            themes,
            workspace,
            tabs: Vec::new(),
            active: None,
            visible: [None, None],
            focus_group: 0,
            untitled: 0,
            explorer,
            search,
            scm,
            tests,
            solution,
            test_runs: 0,
            git_head: None,
            output,
            palette: None,
            palette_restore_theme: None,
            symbol_task: None,
            prompt: None,
            side: SideView::Explorer,
            side_visible: true,
            side_width: 0.0,
            panel: PanelView::Problems,
            references: None,
            panel_visible: false,
            panel_height: 0.0,
            panel_width: 0.0,
            sash: None,
            menu_open: None,
            lsp: None,
            lsp_status: String::new(),
            lsp_task: None,
            dotnet: None,
            dotnet_task: None,
            build_problems: Vec::new(),
            debug: None,
            debug_generation: 0,
            breakpoints: BTreeMap::new(),
            debug_console,
            debug_input,
            terminals: Vec::new(),
            active_terminal: 0,
            plugins: Vec::new(),
            modules: Vec::new(),
            registry: modules::RegistryState::Idle,
            module_busy: None,
            module_filter,
            registry_task: None,
            md_cache: HashMap::new(),
            plugin_tx: None,
            formatters: Vec::new(),
            format_requests: HashMap::new(),
            next_format_request: 1,
            plugin_commands: HashMap::new(),
            plugin_status: Vec::new(),
            module_views: Vec::new(),
            module_menu: None,
            module_view: None,
            module_view_shown: None,
            output_tx,
            focus: cx.focus_handle(),
            theme_version,
            force_close: false,
            custom_titlebar,
            _tasks: tasks,
            _subs: subs,
        };
        wb.side_width = Theme::global(cx).n("sideBar.width", 280.0);
        wb.panel_height = Theme::global(cx).n("panel.height", 240.0);
        wb.panel_width = Theme::global(cx).n("panel.width", 420.0);
        {
            let theme = Theme::global(cx);
            let lay = &theme.layout;
            wb.side_visible = lay.side_visible;
            wb.panel_visible = lay.panel_visible;
            wb.side = side_view_named(&lay.side_default).unwrap_or(SideView::Explorer);
        }
        let auto_reveal = wb.settings.get("explorer.autoReveal", true);
        wb.explorer.update(cx, |e, _| e.auto_reveal = auto_reveal);
        wb.apply_window_effects(window, cx);
        wb.start_plugins(event_tx, cx);

        let entity = cx.entity();
        window.on_window_should_close(cx, move |window, cx| entity.update(cx, |wb, cx| wb.should_close(window, cx)));

        // `byteforge [folder] [file…]`: the first folder opens as the workspace, files open in tabs.
        let mut targets: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
        if targets.is_empty()
            && let Some(last) = wb.state.get("lastFolder")
        {
            targets.push(last);
        }
        let mut folder_opened = false;
        for target in targets {
            let path = normalize_path(Path::new(&target));
            if path.is_dir() && !folder_opened {
                wb.open_folder(path, window, cx);
                folder_opened = true;
            } else if path.is_file() {
                wb.open_file(path, None, window, cx);
            }
        }
        window.focus(&wb.focus);
        wb.log(format!("ByteForge started (theme: {})", Theme::global(cx).data.name));
        wb
    }

    fn log(&self, message: impl std::fmt::Display) {
        let _ = self.output_tx.send(log_line(message));
    }

    fn active_editor(&self) -> Option<Entity<EditorView>> {
        self.active.and_then(|i| self.tabs.get(i)).map(|t| t.editor.clone())
    }

    // ───────────── themes & settings ─────────────

    fn apply_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.themes.apply(name) {
            Ok(data) => {
                self.theme_version += 1;
                assets::set_icon_overrides(data.icons.clone().into_iter().collect());
                let previous = Theme::global(cx);
                let theme = Theme::build(data, self.theme_version, cx);
                // A theme that changes the initial visibility of the side bar or panel applies it on switch.
                if theme.layout.side_visible != previous.layout.side_visible {
                    self.side_visible = theme.layout.side_visible;
                }
                if theme.layout.panel_visible != previous.layout.panel_visible {
                    self.panel_visible = theme.layout.panel_visible;
                }
                theme.install(cx);
                self.apply_window_effects(window, cx);
                for p in &self.plugins {
                    p.event("themeChanged", None);
                }
                cx.notify();
            }
            Err(e) => self.log(format!("Failed to load theme '{name}': {e}")),
        }
    }

    fn reload_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = Theme::global(cx).data.name.clone();
        self.apply_theme(&name, window, cx);
    }

    fn apply_window_effects(&self, window: &mut Window, cx: &App) {
        let appearance = match Theme::global(cx).effect("window.backdrop").map(str::to_lowercase).as_deref() {
            Some("mica" | "acrylic" | "blur") => WindowBackgroundAppearance::Blurred,
            Some("transparent") => WindowBackgroundAppearance::Transparent,
            _ => WindowBackgroundAppearance::Opaque,
        };
        window.set_background_appearance(appearance);
    }

    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme: String = self.settings.get("workbench.theme", "ByteForge Dark".to_string());
        if !theme.eq_ignore_ascii_case(&Theme::global(cx).data.name) {
            self.apply_theme(&theme, window, cx);
        }
        self.workspace.set_excludes(self.settings.get("files.exclude", Vec::<String>::new()));
        let preferred = self.settings.get_string("dotnet.project");
        self.solution.update(cx, |s, _| s.set_preferred(preferred));
        let all = self.settings.all();
        for p in &self.plugins {
            p.settings_changed(&all);
        }
        let auto_reveal = self.settings.get("explorer.autoReveal", true);
        self.explorer.update(cx, |e, _| e.auto_reveal = auto_reveal);
        let overrides: HashMap<String, String> = self.settings.get("keybindings", HashMap::new());
        let mut bindings = Vec::new();
        let commands = cx.global::<Commands>();
        for (id, key) in &overrides {
            if let Some(cmd) = commands.get(id)
                && !key.trim().is_empty()
                && let Ok(b) = KeyBinding::load(key, cmd.action.boxed_clone(), None, false, None, cx.keyboard_mapper().as_ref())
            {
                bindings.push(b);
            }
        }
        cx.bind_keys(bindings);
        let inlay_hints = self.settings.get("editor.inlayHints", true);
        for tab in &self.tabs {
            tab.editor.update(cx, |e, cx| {
                let was = e.inlay_hints_enabled;
                self.configure_editor(e);
                // Applied through the setter, so hints appear or go away right away.
                e.inlay_hints_enabled = was;
                e.set_inlay_hints(inlay_hints, cx);
                cx.notify();
            });
        }
        self.explorer.update(cx, |e, cx| e.refresh(cx));
        cx.notify();
    }

    fn configure_editor(&self, e: &mut EditorView) {
        let s = &self.settings;
        e.state.tab_size = s.get("editor.tabSize", 4usize).max(1);
        e.state.insert_spaces = s.get("editor.insertSpaces", true);
        e.state.auto_close = s.get("editor.autoClosingBrackets", true);
        e.show_line_numbers = s.get("editor.lineNumbers", true);
        e.show_indent_guides = s.get("editor.indentGuides", true);
        e.smooth_scrolling = s.get("editor.smoothScrolling", true);
        e.cursor_blinking = s.get("editor.cursorBlinking", true);
        e.inlay_hints_enabled = s.get("editor.inlayHints", true);
        e.folding_enabled = s.get("editor.folding", true);
        e.minimap_enabled = s.get("editor.minimap", true) && !e.read_only;
        e.snippets = match (e.language, e.read_only) {
            (Some(language), false) => std::sync::Arc::new(bf_core::snippets::for_language(&self.paths.user_snippets(), language.id())),
            _ => std::sync::Arc::new(Vec::new()),
        };
    }

    // ───────────── folders & files ─────────────

    fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let tx = self.event_sender_for_files(cx);
        if let Err(e) = self.workspace.open(path.clone(), move |_| {
            let _ = tx.send(AppEvent::FilesChanged);
        }) {
            self.log(format!("Could not open folder: {e}"));
            return;
        }
        self.state.set("lastFolder", Some(&path.to_string_lossy()));
        self.explorer.update(cx, |e, cx| e.refresh(cx));
        self.scm.update(cx, |s, cx| s.open_folder(cx));
        self.tests.update(cx, |t, cx| t.open_folder(cx));
        self.solution.update(cx, |s, cx| s.open_folder(cx));
        self.git_head = None;
        self.side = side_view_named(&Theme::global(cx).layout.side_default).unwrap_or(SideView::Explorer);
        self.build_problems.clear();
        for view in [&self.output, &self.debug_console] {
            view.update(cx, |e, _| {
                e.link_root = Some(path.clone());
                e.link_cache.clear();
            });
        }
        self.load_breakpoints(cx);
        self.start_lsp(cx);
        for p in &self.plugins {
            p.event("folderOpened", Some(&path));
        }
        self.log(format!("Opened folder {}", path.display()));
        let _ = window;
        cx.notify();
    }

    fn event_sender_for_files(&self, cx: &mut Context<Self>) -> flume::Sender<AppEvent> {
        // A dedicated channel per folder; its receiver forwards into the explorer refresh.
        let (tx, rx) = flume::unbounded::<AppEvent>();
        cx.spawn(async move |this, cx| {
            while let Ok(AppEvent::FilesChanged) = rx.recv_async().await {
                let alive = this.update(cx, |wb, cx| {
                    wb.explorer.update(cx, |e, cx| e.refresh(cx));
                    wb.scm.update(cx, |s, cx| s.refresh(cx));
                    wb.reload_changed_tabs(cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        tx
    }

    pub fn open_file(&mut self, path: PathBuf, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalize_path(&path);
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
            self.activate(i, window, cx);
        } else {
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    self.log(format!("Could not open {}: {e}", path.display()));
                    return;
                }
            };
            let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
            let body = if bom { &bytes[3..] } else { &bytes[..] };
            if body.iter().take(8000).any(|b| *b == 0) {
                self.log(format!("{} looks like a binary file and was not opened.", path.display()));
                self.show_panel(PanelView::Output, cx);
                return;
            }
            let text = String::from_utf8_lossy(body).into_owned();
            let editor = cx.new(|cx| EditorView::new(&text, Some(path.clone()), cx));
            editor.update(cx, |e, cx| {
                self.configure_editor(e);
                if e.language == Some(bf_syntax::Language::CSharp)
                    && let Some(lsp) = &self.lsp
                {
                    e.attach_lsp(lsp.clone(), cx);
                }
            });
            self.prepare_debuggable_editor(&editor, &path, cx);
            let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.add_tab(editor.clone(), Some(path.clone()), title, bom, window, cx);
            let marks = self.tests.read(cx).marks_for(&path);
            editor.update(cx, |e, cx| e.set_test_marks(marks, cx));
            self.load_git_base(editor, path.clone(), cx);
        }
        if let (Some(at), Some(editor)) = (at, self.active_editor()) {
            editor.update(cx, |e, cx| e.go_to(at, cx));
        }
    }

    fn on_output_event(&mut self, _: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let EditorEvent::Navigate(path, at) = event {
            self.open_file(path.clone(), Some(*at), window, cx);
        }
    }

    fn on_editor_event(&mut self, editor: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            EditorEvent::Navigate(path, point) => self.open_file(path.clone(), Some(*point), window, cx),
            EditorEvent::BreakpointsChanged => self.on_editor_breakpoints_changed(editor, cx),
            EditorEvent::ApplyCodeAction(action) => self.apply_code_action((**action).clone(), window, cx),
            EditorEvent::Locations { title, locations } => self.show_references(title.clone(), locations, cx),
            EditorEvent::RenameRequested { symbol, at } => {
                let kind = PromptKind::RenameSymbol { editor: editor.downgrade(), at: *at, old: symbol.clone() };
                self.open_prompt(kind, symbol, window, cx);
            }
            EditorEvent::ApplyWorkspaceEdit(edit) => self.apply_rename(edit, window, cx),
            EditorEvent::Log(text) => {
                self.log(text);
                self.show_panel(PanelView::Output, cx);
            }
            EditorEvent::RunTestAt(line) => {
                if let Some(path) = self.tabs.iter().find(|t| t.editor == *editor).and_then(|t| t.path.clone()) {
                    self.run_test_at(&path, *line, false, window, cx);
                }
            }
            _ => cx.notify(),
        }
    }

    fn add_tab(&mut self, editor: Entity<EditorView>, path: Option<PathBuf>, title: String, bom: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.tabs.push(Tab { editor, group: self.focus_group, path, title, bom, page: None, _subs: vec![sub] });
        let ix = self.tabs.len() - 1;
        self.activate(ix, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active = Some(ix);
            let group = self.tabs[ix].group;
            self.visible[group] = Some(ix);
            self.focus_group = group;
            let editor = self.tabs[ix].editor.clone();
            editor.update(cx, |e, _| e.focus(window));
            let path = self.tabs[ix].path.clone();
            self.explorer.update(cx, |e, cx| e.set_active(path.as_deref(), cx));
            self.notify_editor_changed();
            cx.notify();
        }
    }

    fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.untitled += 1;
        let editor = cx.new(|cx| EditorView::new("", None, cx));
        editor.update(cx, |e, _| self.configure_editor(e));
        let title = format!("Untitled-{}", self.untitled);
        self.add_tab(editor, None, title, false, window, cx);
    }

    fn save_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.editor.read(cx).read_only {
            return;
        }
        let Some(path) = tab.path.clone() else {
            self.save_as(ix, window, cx);
            return;
        };
        let text = tab.editor.read(cx).save_path_text();
        let mut bytes = Vec::with_capacity(text.len() + 3);
        if tab.bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(text.as_bytes());
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                tab.editor.update(cx, |e, cx| {
                    e.state.buffer.mark_saved();
                    e.did_save();
                    cx.notify();
                });
                for p in &self.plugins {
                    p.event("fileSaved", Some(&path));
                }
                self.scm.update(cx, |s, cx| s.refresh(cx));
                if self.themes.current().file_path.as_deref() == Some(path.as_path()) || path.starts_with(self.paths.user_themes()) {
                    self.reload_theme(window, cx);
                }
            }
            Err(e) => {
                self.log(format!("Could not save {}: {e}", path.display()));
                self.show_panel(PanelView::Output, cx);
            }
        }
        cx.notify();
    }

    fn save_as(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let dir = tab.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.workspace.root()).unwrap_or_else(|| PathBuf::from("."));
        let name = tab.title.clone();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    let _ = std::fs::write(&path, "");
                    let path = normalize_path(&path);
                    if let Some(tab) = wb.tabs.get_mut(ix) {
                        tab.title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        tab.path = Some(path.clone());
                        let text = tab.editor.read(cx).state.buffer.text();
                        // Recreate the editor so highlighting and language features match the new file type.
                        let editor = cx.new(|cx| EditorView::new(&text, Some(path.clone()), cx));
                        editor.update(cx, |e, cx| {
                            wb.configure_editor(e);
                            if e.language == Some(bf_syntax::Language::CSharp)
                                && let Some(lsp) = &wb.lsp
                            {
                                e.attach_lsp(lsp.clone(), cx);
                            }
                        });
                        wb.prepare_debuggable_editor(&editor, &path, cx);
                        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
                        let tab = &mut wb.tabs[ix];
                        tab.editor = editor;
                        tab._subs = vec![sub];
                    }
                    wb.save_tab(ix, window, cx);
                    wb.activate(ix, window, cx);
                });
            }
        })
        .detach();
    }

    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            if tab.path.is_some() && tab.editor.read(cx).is_modified() {
                self.save_tab(ix, window, cx);
            }
        }
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.editor.read(cx).is_modified() {
            let title = tab.title.clone();
            let rx = window.prompt(PromptLevel::Warning, &format!("Do you want to save the changes you made to {title}?"), None, &["Save", "Don't Save", "Cancel"], cx);
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(choice) = rx.await {
                    let _ = this.update_in(cx, |wb, window, cx| match choice {
                        0 => {
                            wb.save_tab(ix, window, cx);
                            wb.remove_tab(ix, window, cx);
                        }
                        1 => wb.remove_tab(ix, window, cx),
                        _ => {}
                    });
                }
            })
            .detach();
            return;
        }
        self.remove_tab(ix, window, cx);
    }

    fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        tab.editor.update(cx, |e, _| e.detach_lsp());
        for slot in &mut self.visible {
            *slot = match *slot {
                Some(j) if j == ix => None,
                Some(j) if j > ix => Some(j - 1),
                other => other,
            };
        }
        // The group the tab was in shows its neighbour (the tab before it, else the next one).
        if self.visible[tab.group].is_none() {
            let in_group: Vec<usize> = self.group_tabs(tab.group);
            self.visible[tab.group] = in_group.iter().rev().find(|&&j| j < ix).or(in_group.first()).copied();
        }
        self.normalize_groups();
        self.active = self.visible[self.focus_group];
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        } else {
            window.focus(&self.focus);
            self.explorer.update(cx, |e, cx| e.set_active(None, cx));
            self.notify_editor_changed();
        }
        cx.notify();
    }

    /// The file of the active tab (None for untitled documents and pages).
    fn active_path(&self) -> Option<PathBuf> {
        self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.path.clone())
    }

    /// Tells modules which file is in front (`editorChanged`, path null when none is).
    fn notify_editor_changed(&self) {
        let path = self.active_path();
        for p in &self.plugins {
            p.event("editorChanged", path.as_deref());
        }
    }

    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.force_close {
            self.shutdown(cx);
            return true;
        }
        let dirty = self.tabs.iter().filter(|t| t.editor.read(cx).is_modified()).count();
        if dirty == 0 {
            self.shutdown(cx);
            return true;
        }
        let rx = window.prompt(PromptLevel::Warning, &format!("{dirty} file(s) have unsaved changes."), None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(choice) = rx.await
                && choice != 2
            {
                let _ = this.update_in(cx, |wb, window, cx| {
                    if choice == 0 {
                        wb.save_all(window, cx);
                    }
                    wb.force_close = true;
                    wb.shutdown(cx);
                    cx.quit();
                });
            }
        })
        .detach();
        false
    }

    fn shutdown(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.editor.update(cx, |e, _| e.detach_lsp());
        }
        if let Some(lsp) = self.lsp.take() {
            lsp.shutdown();
        }
        if let Some(d) = &self.dotnet {
            d.kill();
        }
        if let Some(session) = self.debug.take()
            && let Some(client) = session.client.clone()
        {
            // End a launched program properly (an attached one is detached), then stop the adapter.
            let _ = client.disconnect(!session.attached).recv_timeout(std::time::Duration::from_millis(1500));
            client.kill();
        }
        self.terminals.clear();
        for p in &self.plugins {
            p.stop();
        }
    }

    // ───────────── C# language server ─────────────

    fn start_lsp(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if let Some(old) = self.lsp.take() {
            for tab in &self.tabs {
                tab.editor.update(cx, |e, _| e.detach_lsp());
            }
            old.shutdown();
        }
        let has_csharp = self.workspace.files().iter().any(|f| matches!(f.extension().and_then(|e| e.to_str()), Some("cs" | "csproj" | "sln" | "slnx")));
        if !has_csharp {
            self.lsp_status = String::new();
            return;
        }
        let command = self.settings.get_string("csharp.languageServer").or_else(bf_lsp::csharp_ls_command);
        let Some(command) = command else {
            self.lsp_status = "C#: no server".into();
            self.log("csharp-ls not found. Install it with: dotnet tool install -g csharp-ls (or set \"csharp.languageServer\").");
            return;
        };
        match LspClient::start(ServerConfig { command, args: Vec::new(), root, language_id: "csharp".into() }) {
            Ok((client, events)) => {
                self.lsp_status = "C#: starting…".into();
                for tab in &self.tabs {
                    let client = client.clone();
                    tab.editor.update(cx, |e, cx| {
                        if e.language == Some(bf_syntax::Language::CSharp) {
                            e.attach_lsp(client, cx);
                        }
                    });
                }
                self.lsp = Some(client);
                self.lsp_task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv_async().await {
                        let ok = this
                            .update(cx, |wb, cx| {
                                match event {
                                    LspEvent::Ready => wb.lsp_status = "C#: loading…".into(),
                                    LspEvent::Progress(text) => {
                                        wb.lsp_status = if text.contains("Finished") { "C#: ready".into() } else { format!("C#: {}", short(&text, 48)) };
                                    }
                                    LspEvent::Diagnostics { path, diagnostics, .. } => {
                                        if let Some(tab) = wb.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
                                            tab.editor.update(cx, |e, cx| e.set_lsp_diagnostics(diagnostics, cx));
                                        }
                                    }
                                    LspEvent::Log(text) => {
                                        if text.to_lowercase().contains("error") {
                                            wb.log(format!("[csharp-ls] {text}"));
                                        }
                                    }
                                    LspEvent::ApplyEdit(edit) => wb.apply_workspace_edit(&edit, None, cx),
                                    LspEvent::Exited(reason) => {
                                        wb.lsp_status = "C#: stopped".into();
                                        wb.log(format!("C# language server exited: {reason}"));
                                    }
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !ok {
                            break;
                        }
                    }
                }));
            }
            Err(e) => {
                self.lsp_status = "C#: failed".into();
                self.log(format!("Could not start the C# language server: {e}"));
            }
        }
        cx.notify();
    }

    // ───────────── code actions ─────────────

    /// Applies a quick fix or refactoring: resolves its edit if the server computes it lazily, applies the
    /// edit, then runs its command.
    fn apply_code_action(&mut self, action: bf_lsp::CodeAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lsp) = self.lsp.clone() else { return };
        // Lazily computed actions carry `data` instead of an edit.
        let resolve = action.edit.is_none() && (action.needs_resolve() || action.raw.get("data").is_some_and(|d| !d.is_null()));
        if !resolve {
            self.finish_code_action(action, window, cx);
            return;
        }
        let rx = lsp.resolve_code_action(&action);
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update_in(cx, |wb, window, cx| match result {
                Ok(Ok(resolved)) => wb.finish_code_action(resolved, window, cx),
                Ok(Err(e)) => wb.log(format!("Could not apply \"{}\": {e:#}", action.title)),
                Err(_) => {}
            });
        })
        .detach();
    }

    fn finish_code_action(&mut self, action: bf_lsp::CodeAction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &action.edit {
            self.apply_workspace_edit(edit, Some(window), cx);
        }
        if let (Some(command), Some(lsp)) = (&action.command, &self.lsp) {
            let _ = lsp.execute_command(command);
        }
        if action.edit.is_none() && action.command.is_none() {
            self.log(format!("\"{}\" made no changes.", action.title));
        }
    }

    /// Applies a language-server edit: open files change in their editor (one undo step each). With a
    /// window, other files are opened first so the change can be reviewed and saved; without one they are
    /// changed on disk.
    fn apply_workspace_edit(&mut self, edit: &bf_lsp::WorkspaceEdit, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        for path in &edit.creates {
            if !path.exists() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(path, "") {
                    self.log(format!("Could not create {}: {e}", path.display()));
                }
            }
        }
        let active = self.active;
        for (path, edits) in &edit.changes {
            let path = normalize_path(path);
            let find = |wb: &Self| wb.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))).map(|t| t.editor.clone());
            let mut editor = find(self);
            if editor.is_none()
                && let Some(window) = window.as_deref_mut()
            {
                self.open_file(path.clone(), None, window, cx);
                editor = find(self);
            }
            match editor {
                Some(editor) => editor.update(cx, |e, cx| e.apply_text_edits(edits, cx)),
                None => {
                    let result = std::fs::read_to_string(&path).and_then(|text| {
                        let mut buffer = bf_text::Buffer::new(&text);
                        let mut points: Vec<_> = edits
                            .iter()
                            .map(|e| {
                                let start = buffer.utf16_to_point(e.range.start.line as usize, e.range.start.character as usize);
                                let end = buffer.utf16_to_point(e.range.end.line as usize, e.range.end.character as usize);
                                (start, end, e.new_text.replace("\r\n", "\n"))
                            })
                            .collect();
                        points.sort_by_key(|p| std::cmp::Reverse(p.0));
                        for (start, end, text) in points {
                            buffer.replace(start, end, &text);
                        }
                        std::fs::write(&path, buffer.text_for_save())
                    });
                    if let Err(e) = result {
                        self.log(format!("Could not change {}: {e}", path.display()));
                    }
                }
            }
        }
        // Opening files to edit them shouldn't move you away from where you were.
        if let (Some(ix), Some(window)) = (active, window) {
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    // ───────────── dotnet ─────────────

    fn run_dotnet(&mut self, verb: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder containing a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        if self.dotnet_busy(cx) {
            return;
        }
        let runnable = verb == "run";
        let configured = self.settings.get_string("dotnet.project");
        let Some(target) = find_target(&root, configured.as_deref(), runnable) else {
            self.log(format!("No .sln, .slnx or .csproj found in {}.", root.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.save_all(window, cx);
        let mut args = vec![verb.to_owned()];
        if runnable {
            args.extend(["--project".into(), target.to_string_lossy().into_owned()]);
        } else {
            args.push(target.to_string_lossy().into_owned());
        }
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet(root, args, None, window, cx);
    }

    /// Logs and returns true when a dotnet process (or a debug session) is already running.
    fn dotnet_busy(&mut self, cx: &mut Context<Self>) -> bool {
        let busy = self.dotnet.as_ref().is_some_and(DotnetProcess::is_running) || self.debug.is_some();
        if busy {
            self.log("A dotnet process or debug session is already running. Stop it first (Shift+F5).");
            self.show_panel(PanelView::Output, cx);
        }
        busy
    }

    /// Runs `dotnet <args>` in `root` with output streamed to the Output panel. Compiler errors and
    /// warnings go to Problems. `then` runs when the process exits, with whether it succeeded.
    fn start_dotnet(&mut self, root: PathBuf, args: Vec<String>, then: Option<DotnetDone>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.start_dotnet_with(root, args, Vec::new(), None, then, window, cx)
    }

    /// [`start_dotnet`](Self::start_dotnet) with extra environment variables and a hook called with each
    /// output line.
    #[allow(clippy::too_many_arguments)]
    fn start_dotnet_with(
        &mut self,
        root: PathBuf,
        args: Vec<String>,
        env: Vec<(String, String)>,
        mut on_line: Option<LineHook>,
        then: Option<DotnetDone>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let verb = args.first().cloned().unwrap_or_default();
        let compiles = matches!(verb.as_str(), "build" | "run" | "test");
        if compiles {
            self.build_problems.clear();
        }
        self.log(format!("> dotnet {}", args.join(" ")));
        match DotnetProcess::start_with_env(&root, args, &env) {
            Ok((process, rx)) => {
                self.dotnet = Some(process);
                let tx = self.output_tx.clone();
                self.dotnet_task = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = rx.recv_async().await {
                        match event {
                            RunEvent::Line(line) => {
                                if compiles && let Some(d) = bf_core::debug::parse_build_line(&line, &root) {
                                    let _ = this.update(cx, |wb, _| wb.add_build_problem(d));
                                }
                                if let Some(hook) = on_line.as_mut() {
                                    let _ = this.update_in(cx, |wb, window, cx| hook(wb, &line, window, cx));
                                }
                                let _ = tx.send(line);
                            }
                            RunEvent::Exited { code, seconds } => {
                                let _ = tx.send(log_line(format!("dotnet {verb} exited with code {} after {seconds:.1}s", code.map_or("?".into(), |c| c.to_string()))));
                                let _ = this.update_in(cx, |wb, window, cx| {
                                    wb.dotnet = None;
                                    if compiles {
                                        wb.apply_build_diagnostics(cx);
                                    }
                                    if let Some(then) = then {
                                        then(wb, code == Some(0), window, cx);
                                    }
                                    cx.notify();
                                });
                                break;
                            }
                        }
                    }
                }));
                cx.notify();
                true
            }
            Err(e) => {
                self.log(format!("Could not start dotnet (is the .NET SDK installed?): {e}"));
                cx.notify();
                false
            }
        }
    }

    fn add_build_problem(&mut self, d: bf_core::debug::BuildDiagnostic) {
        let line = d.line.saturating_sub(1) as usize;
        let col = d.column.saturating_sub(1) as usize;
        let diagnostic = crate::editor::Diagnostic {
            start: Point::new(line, col),
            end: Point::new(line, col + 1),
            severity: match d.severity {
                bf_core::debug::BuildSeverity::Error => bf_lsp::Severity::Error,
                bf_core::debug::BuildSeverity::Warning => bf_lsp::Severity::Warning,
            },
            message: d.message,
            code: d.code,
        };
        let path = normalize_path(&d.path);
        // MSBuild repeats every diagnostic in its summary.
        let duplicate = self.build_problems.iter().any(|(p, x)| {
            *p == path && x.start == diagnostic.start && x.message == diagnostic.message && x.code == diagnostic.code
        });
        if !duplicate {
            self.build_problems.push((path, diagnostic));
        }
    }

    /// Whether build diagnostics stand in for a language server in this editor (Razor files, or C#
    /// without csharp-ls).
    fn uses_build_diagnostics(&self, editor: &EditorView) -> bool {
        editor.language != Some(bf_syntax::Language::CSharp) || self.lsp.is_none()
    }

    /// Shows the last build's diagnostics in open editors that have no language server.
    fn apply_build_diagnostics(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let Some(path) = &tab.path else { continue };
            if !self.uses_build_diagnostics(tab.editor.read(cx)) {
                continue;
            }
            let diagnostics: Vec<_> = self.build_problems.iter().filter(|(p, _)| p == path).map(|(_, d)| d.clone()).collect();
            tab.editor.update(cx, |e, cx| e.set_diagnostics(diagnostics, cx));
        }
    }

    // ───────────── terminal ─────────────

    /// Starts a shell in the opened folder ("terminal.shell" and "terminal.args" in settings, else $SHELL).
    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shell = crate::terminal::default_shell(self.settings.get_string("terminal.shell"));
        let args: Vec<String> = self.settings.get("terminal.args", Vec::new());
        let cwd = self.workspace.root().or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let spawned = match crate::terminal::spawn(shell, args, cwd) {
            Ok(spawned) => spawned,
            Err(e) => {
                self.log(format!("Could not start the terminal: {e:#}"));
                self.show_panel(PanelView::Output, cx);
                return;
            }
        };
        let terminal = cx.new(|cx| TerminalView::new(spawned, cx));
        let sub = cx.subscribe_in(&terminal, window, |wb, terminal, event, window, cx| match event {
            TerminalEvent::Exited => {
                // Like VS Code: a terminal whose shell exits goes away.
                if let Some(ix) = wb.terminals.iter().position(|(t, _)| t == terminal) {
                    wb.remove_terminal(ix, window, cx);
                }
            }
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::OpenFile(path, line, column) => {
                let at = Point::new(line.saturating_sub(1) as usize, column.saturating_sub(1) as usize);
                wb.open_file(path.clone(), Some(at), window, cx);
            }
        });
        self.terminals.push((terminal, sub));
        self.active_terminal = self.terminals.len() - 1;
        self.show_terminal(window, cx);
    }

    /// Shows the terminal panel (starting a terminal if none is open) and focuses it.
    fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            self.new_terminal(window, cx);
            return;
        }
        self.show_panel(PanelView::Terminal, cx);
        if let Some((terminal, _)) = self.terminals.get(self.active_terminal) {
            terminal.read(cx).focus(window);
        }
    }

    /// Ctrl+`: shows and focuses the terminal, or hides the panel when the terminal is focused.
    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.terminals.get(self.active_terminal).is_some_and(|(t, _)| t.focus_handle(cx).contains_focused(window, cx));
        if self.panel_visible && self.panel == PanelView::Terminal && focused {
            self.panel_visible = false;
            match self.active_editor() {
                Some(editor) => editor.update(cx, |e, _| e.focus(window)),
                None => window.focus(&self.focus),
            }
            cx.notify();
        } else {
            self.show_terminal(window, cx);
        }
    }

    /// Lists the installed shells; the pick is saved as "terminal.shell".
    fn open_shell_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.settings.get_string("terminal.shell");
        let login = std::env::var("SHELL").unwrap_or_default();
        let mut default = PaletteItem::new("Login shell", PaletteAction::Command(Box::new(SetDefaultShell { shell: String::new() })));
        default.detail = Some(if login.is_empty() { "the system default".into() } else { login.clone() });
        default.hint = current.is_none().then(|| "current".into());
        let mut items = vec![default];
        for shell in crate::terminal::installed_shells() {
            let name = Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| shell.clone());
            let mut item = PaletteItem::new(name, PaletteAction::Command(Box::new(SetDefaultShell { shell: shell.clone() })));
            item.detail = Some(shell.clone());
            item.hint = (current.as_deref() == Some(shell.as_str())).then(|| "current".into());
            item.search = Some(shell);
            items.push(item);
        }
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, "Select the default shell for new terminals", "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    fn kill_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminals.is_empty() {
            self.remove_terminal(self.active_terminal, window, cx);
        }
    }

    fn remove_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.terminals.len() {
            return;
        }
        drop(self.terminals.remove(ix));
        self.active_terminal = self.active_terminal.min(self.terminals.len().saturating_sub(1));
        if self.terminals.is_empty() && self.panel == PanelView::Terminal {
            self.panel_visible = false;
            match self.active_editor() {
                Some(editor) => editor.update(cx, |e, _| e.focus(window)),
                None => window.focus(&self.focus),
            }
        } else if self.panel == PanelView::Terminal
            && let Some((terminal, _)) = self.terminals.get(self.active_terminal)
        {
            terminal.read(cx).focus(window);
        }
        cx.notify();
    }

    /// The terminal switcher in the panel header: one chip per terminal, then new and kill buttons.
    fn render_terminal_tabs(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::global(cx);
        let hover = t.c("toolbar.hoverBackground");
        let icon_size = t.m("panel.iconSize", 14.0);
        let button = |id: &'static str, name: &'static str, tip: &'static str| {
            div().id(id).p_1().rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).tooltip(tooltip(tip)).child(icon(name, icon_size, t.c("icon.foreground")))
        };
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        if self.terminals.len() > 1 {
            for (i, (terminal, _)) in self.terminals.iter().enumerate() {
                let active = i == self.active_terminal;
                out.push(
                    div()
                        .id(SharedString::from(format!("term-{i}")))
                        .mx_1()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .when(active, |d| d.bg(t.c("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                        .when(!active, |d| d.text_color(t.c("panelTitle.inactiveForeground")).hover(move |s| s.bg(hover)))
                        .child(format!("{}: {}", i + 1, short(terminal.read(cx).display_title(), 24)))
                        .on_click(cx.listener(move |wb, _, w, cx| {
                            wb.active_terminal = i;
                            wb.show_terminal(w, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        out.push(button("term-new", "add", "New Terminal (Ctrl+Shift+`)").on_click(cx.listener(|wb, _, w, cx| wb.new_terminal(w, cx))).into_any_element());
        out.push(button("term-shell", "terminal", "Select Default Shell…").on_click(cx.listener(|wb, _, w, cx| wb.open_shell_picker(w, cx))).into_any_element());
        out.push(button("term-kill", "trash", "Kill Terminal").on_click(cx.listener(|wb, _, w, cx| wb.kill_terminal(w, cx))).into_any_element());
        out
    }

    // ───────────── modules ─────────────

    fn start_plugins(&mut self, _event_tx: flume::Sender<AppEvent>, cx: &mut Context<Self>) {
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        self.preinstall_modules();
        self.modules = discover(&self.paths.user_modules());
        let (tx, rx) = flume::unbounded::<PluginMessage>();
        self.plugin_tx = Some(tx);
        for manifest in self.modules.clone() {
            if disabled.contains(&manifest.id) {
                self.log(format!("Module {} ({}) is disabled.", manifest.name, manifest.id));
                continue;
            }
            self.start_module(manifest);
        }
        self.refresh_module_contributions(cx);
        self._tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(message) = rx.recv_async().await {
                if this.update(cx, |wb, cx| wb.on_plugin_message(message, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn start_module(&mut self, manifest: ModuleManifest) {
        if manifest.command.trim().is_empty() {
            return; // only declares things (file icons), nothing to run
        }
        let Some(tx) = self.plugin_tx.clone() else { return };
        match PluginHandle::start(manifest.clone(), tx) {
            Ok(handle) => {
                handle.activate(self.workspace.root().as_deref(), &self.settings.all());
                // A restarted module learns again that its view is visible.
                if self.module_view_shown.as_ref().is_some_and(|(m, _)| *m == manifest.id) {
                    self.module_view_shown = None;
                }
                handle.event("editorChanged", self.active_path().as_deref());
                self.log(format!("Module '{}' started", manifest.name));
                self.plugins.push(handle);
            }
            Err(e) => self.log(format!("Module '{}' failed to start: {e}", manifest.name)),
        }
    }

    /// Stops a module and drops everything it contributed (formatters, commands, status items).
    fn stop_module(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.plugins.iter().position(|p| p.manifest.id == id) {
            self.plugins.remove(i).stop();
        }
        self.formatters.retain(|f| f.module != id);
        self.plugin_status.retain(|s| s.module != id);
        let commands: Vec<String> = self.plugin_commands.iter().filter(|(_, m)| *m == id).map(|(c, _)| c.clone()).collect();
        for command in &commands {
            self.plugin_commands.remove(command);
        }
        cx.global_mut::<Commands>().list.retain(|c| !commands.contains(&c.id));
        cx.notify();
    }

    // ───────────── formatting ─────────────

    /// The formatter for a file: "editor.defaultFormatter" if it handles the file, else the first module
    /// formatter that does, else the C# language server. Returns (id, name).
    fn formatter_for(&self, editor: &EditorView) -> Option<(String, String)> {
        let ext = editor.path.as_deref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let handles = |f: &&Formatter| f.extensions.contains(&ext);
        let lsp = (editor.language == Some(bf_syntax::Language::CSharp) && self.lsp.is_some()).then(|| ("csharp-ls".to_owned(), "csharp-ls".to_owned()));
        let preferred = self.settings.get_string("editor.defaultFormatter");
        if let Some(preferred) = preferred.as_deref() {
            if preferred == "csharp-ls" && lsp.is_some() {
                return lsp;
            }
            if let Some(f) = self.formatters.iter().filter(handles).find(|f| f.id == preferred) {
                return Some((f.id.clone(), f.name.clone()));
            }
        }
        self.formatters.iter().find(handles).map(|f| (f.id.clone(), f.name.clone())).or(lsp)
    }

    /// Formats a tab with its formatter; `save_after` saves it once formatted (format on save). Returns
    /// false when no formatter handles the file.
    fn format_tab(&mut self, ix: usize, formatter: Option<String>, save_after: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.get(ix) else { return false };
        let editor = tab.editor.clone();
        let (id, name) = match formatter {
            Some(id) => {
                let name = self.formatters.iter().find(|f| f.id == id).map_or_else(|| id.clone(), |f| f.name.clone());
                (id, name)
            }
            None => match self.formatter_for(editor.read(cx)) {
                Some(found) => found,
                None => return false,
            },
        };
        let (path, text, version, tab_size, spaces) = {
            let e = editor.read(cx);
            (e.path.clone(), e.state.buffer.text(), e.state.buffer.version(), e.state.tab_size, e.state.insert_spaces)
        };
        let Some(path) = path else { return false };
        if id == "csharp-ls" {
            let Some(lsp) = self.lsp.clone() else { return false };
            let rx = lsp.formatting(&path, tab_size as u32, spaces);
            let weak = editor.downgrade();
            cx.spawn_in(window, async move |this, cx| {
                let result = rx.recv_async().await;
                let _ = this.update_in(cx, |wb, window, cx| {
                    match result {
                        Ok(Ok(edits)) => {
                            if let Some(editor) = weak.upgrade().filter(|e| e.read(cx).state.buffer.version() == version) {
                                editor.update(cx, |e, cx| e.apply_text_edits(&edits, cx));
                            }
                        }
                        Ok(Err(e)) => wb.log(format!("csharp-ls could not format {}: {e:#}", path.display())),
                        Err(_) => {}
                    }
                    if save_after {
                        wb.save_editor(&weak, window, cx);
                    }
                });
            })
            .detach();
            return true;
        }
        let Some(formatter) = self.formatters.iter().find(|f| f.id == id) else { return false };
        let Some(plugin) = self.plugins.iter().find(|p| p.manifest.id == formatter.module) else { return false };
        let request = self.next_format_request;
        self.next_format_request += 1;
        plugin.format(request, &path, &text);
        self.format_requests.insert(request, PendingFormat { editor: editor.downgrade(), version, save_after, name });
        // A module that never answers mustn't keep a save from happening.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
            let _ = this.update_in(cx, |wb, window, cx| {
                if let Some(pending) = wb.format_requests.remove(&request) {
                    wb.log(format!("{} did not answer in time; the file was not formatted.", pending.name));
                    if pending.save_after {
                        wb.save_editor(&pending.editor, window, cx);
                    }
                }
            });
        })
        .detach();
        true
    }

    fn on_format_result(&mut self, request: u64, result: Result<String, String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.format_requests.remove(&request) else { return };
        if let Some(editor) = pending.editor.upgrade() {
            match result {
                Ok(text) if editor.read(cx).state.buffer.version() == pending.version => editor.update(cx, |e, cx| e.replace_text_minimal(&text, cx)),
                Ok(_) => self.log(format!("The file changed while {} was formatting it; run Format Document again.", pending.name)),
                Err(error) => {
                    self.log(format!("{}: {error}", pending.name));
                    if !pending.save_after {
                        self.show_panel(PanelView::Output, cx);
                    }
                }
            }
        }
        if pending.save_after {
            self.save_editor(&pending.editor, window, cx);
        }
    }

    /// Saves the tab showing `editor`, if it's still open.
    fn save_editor(&mut self, editor: &gpui::WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.editor.downgrade() == *editor) {
            self.save_tab(ix, window, cx);
        }
    }

    /// Ctrl+S: formats first when "editor.formatOnSave" is on and a formatter handles the file.
    fn save_formatted(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let on: bool = self.settings.get("editor.formatOnSave", false);
        let has_path = self.tabs.get(ix).is_some_and(|t| t.path.is_some());
        if !(on && has_path && self.format_tab(ix, None, true, window, cx)) {
            self.save_tab(ix, window, cx);
        }
    }

    fn open_formatter_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let (ext, csharp) = {
            let e = editor.read(cx);
            (e.path.as_deref().and_then(|p| p.extension()).map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default(), e.language == Some(bf_syntax::Language::CSharp))
        };
        let current = self.formatter_for(editor.read(cx)).map(|(id, _)| id);
        let mut items: Vec<PaletteItem> = self
            .formatters
            .iter()
            .filter(|f| f.extensions.contains(&ext))
            .map(|f| {
                let mut item = PaletteItem::new(f.name.clone(), PaletteAction::Command(Box::new(UseFormatter { id: f.id.clone() })));
                item.detail = Some(format!("module {}", f.module));
                item.hint = (current.as_deref() == Some(f.id.as_str())).then(|| "default".into());
                item
            })
            .collect();
        if csharp && self.lsp.is_some() {
            let mut item = PaletteItem::new("csharp-ls (Roslyn formatter)", PaletteAction::Command(Box::new(UseFormatter { id: "csharp-ls".into() })));
            item.hint = (current.as_deref() == Some("csharp-ls")).then(|| "default".into());
            items.push(item);
        }
        if items.is_empty() {
            self.log(format!("No formatter handles .{ext} files."));
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, "Select the formatter (it becomes the default)", "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Runs `f` with the active window once the current update is done. Module messages are handled while the
    /// workbench is already being updated, so updating it again right away (to get a `Window`) would panic.
    fn with_window(cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static) {
        let entity = cx.entity();
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| entity.update(cx, |wb, cx| f(wb, window, cx)));
            }
        });
    }

    fn on_plugin_message(&mut self, message: PluginMessage, cx: &mut Context<Self>) {
        match message {
            PluginMessage::RegisterCommand { module, id, title, category, keybinding } => {
                let action = RunModuleCommand { id: id.clone() };
                if let Some(key) = keybinding
                    && let Ok(b) = KeyBinding::load(&key, Box::new(action.clone()), None, false, None, cx.keyboard_mapper().as_ref())
                {
                    cx.bind_keys([b]);
                }
                let category: &'static str = Box::leak(category.unwrap_or_else(|| "Module".into()).into_boxed_str());
                cx.global_mut::<Commands>().list.push(CommandInfo { id: id.clone(), title, category, action: Box::new(action) });
                self.plugin_commands.insert(id, module);
            }
            PluginMessage::Status { module, id, text, tooltip, command, right } => {
                match self.plugin_status.iter_mut().find(|s| s.id == id) {
                    Some(s) => {
                        s.text = text;
                        s.tooltip = tooltip;
                        s.command = command;
                        s.right = right;
                    }
                    None => self.plugin_status.push(StatusEntry { module, id, text, tooltip, command, right }),
                }
            }
            PluginMessage::Log { module, text } => self.log(format!("[{module}] {text}")),
            PluginMessage::OpenFile { path, line, column } => {
                let point = line.map(|l| Point::new(l, column.unwrap_or(0)));
                Self::with_window(cx, move |wb, window, cx| wb.open_file(path, point, window, cx));
            }
            PluginMessage::InsertText { text } => {
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |e, cx| e.insert_text(&text, cx));
                }
            }
            PluginMessage::SetTheme { name } => {
                self.settings.set("workbench.theme", json!(name));
                Self::with_window(cx, move |wb, window, cx| wb.apply_theme(&name, window, cx));
            }
            PluginMessage::ShowView { id } => match id.as_str() {
                "output" => self.show_panel(PanelView::Output, cx),
                "problems" => self.show_panel(PanelView::Problems, cx),
                "explorer" => self.show_side(SideView::Explorer, cx),
                "search" => self.show_side(SideView::Search, cx),
                other if self.module_views.iter().any(|v| v.id == other) => self.show_module_view(other, cx),
                other => self.show_side(side_view_named(other).unwrap_or(SideView::Modules), cx),
            },
            PluginMessage::SetTree { module, tree } => self.set_module_tree(module, tree),
            PluginMessage::RunInTerminal { title, command, cwd, save_all } => Self::with_window(cx, move |wb, window, cx| {
                if save_all {
                    wb.save_all(window, cx);
                }
                wb.run_in_terminal(title, command, cwd, window, cx);
            }),
            PluginMessage::QuickPick { module, request, placeholder, items } => {
                Self::with_window(cx, move |wb, window, cx| wb.module_quick_pick(module, request, placeholder, items, window, cx));
            }
            PluginMessage::Prompt { module, prompt } => {
                Self::with_window(cx, move |wb, window, cx| wb.module_prompt(module, prompt, window, cx));
            }
            PluginMessage::RegisterFormatter { module, id, name, extensions } => {
                self.formatters.retain(|f| f.id != id);
                self.formatters.push(Formatter { module, id, name, extensions });
            }
            PluginMessage::FormatResult { request, result, .. } => {
                Self::with_window(cx, move |wb, window, cx| wb.on_format_result(request, result, window, cx));
            }
            PluginMessage::Exited { module, reason } => {
                self.formatters.retain(|f| f.module != module);
                self.module_views_stopped(&module);
                self.log(format!("Module {module} stopped: {reason}"));
            }
        }
        cx.notify();
    }

    // ───────────── views ─────────────

    fn show_side(&mut self, view: SideView, cx: &mut Context<Self>) {
        self.side = view;
        self.side_visible = true;
        cx.notify();
    }

    fn toggle_side(&mut self, view: SideView, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_visible && self.side == view {
            self.side_visible = false;
        } else {
            self.show_side(view, cx);
            if view == SideView::Search {
                let focus = self.search.focus_handle(cx);
                window.focus(&focus);
            }
        }
        cx.notify();
    }

    fn show_panel(&mut self, view: PanelView, cx: &mut Context<Self>) {
        self.panel = view;
        self.panel_visible = true;
        cx.notify();
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let in_group = self.group_tabs(self.focus_group);
        if in_group.is_empty() {
            return;
        }
        let pos = self.active.and_then(|a| in_group.iter().position(|&j| j == a)).unwrap_or(0);
        let i = in_group[(pos as isize + delta).rem_euclid(in_group.len() as isize) as usize];
        self.activate(i, window, cx);
    }

    // ───────────── editor groups (split editor) ─────────────

    /// Indexes of the tabs in `group`, in tab order.
    fn group_tabs(&self, group: usize) -> Vec<usize> {
        self.tabs.iter().enumerate().filter(|(_, t)| t.group == group).map(|(i, _)| i).collect()
    }

    fn is_split(&self) -> bool {
        self.tabs.iter().any(|t| t.group == 1)
    }

    /// Keeps the groups consistent: an empty left group takes over the right one, an empty focused
    /// group hands focus to the other.
    fn normalize_groups(&mut self) {
        if self.group_tabs(0).is_empty() && self.is_split() {
            for t in &mut self.tabs {
                t.group = 0;
            }
            self.visible = [self.visible[1], None];
            self.focus_group = 0;
        }
        if self.group_tabs(self.focus_group).is_empty() {
            self.focus_group = 0;
        }
        for g in 0..2 {
            if self.visible[g].is_none_or(|i| self.tabs.get(i).is_none_or(|t| t.group != g)) {
                self.visible[g] = self.group_tabs(g).last().copied();
            }
        }
    }

    /// Ctrl+\\: moves the current editor into the other group (opening the right group when there is
    /// none). The only tab of a group can't leave it unless the other group exists.
    fn split_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let from = self.tabs[ix].group;
        if !self.is_split() && self.group_tabs(0).len() < 2 {
            self.log("Split Editor: open a second file first; the current one moves into the right group.");
            return;
        }
        let to = 1 - from;
        self.tabs[ix].group = to;
        if self.visible[from] == Some(ix) {
            let left: Vec<usize> = self.group_tabs(from);
            self.visible[from] = left.iter().rev().find(|&&j| j < ix).or(left.first()).copied();
        }
        self.visible[to] = Some(ix);
        self.focus_group = to;
        self.normalize_groups();
        if let Some(i) = self.visible[self.focus_group] {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    fn focus_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.visible[group].filter(|_| group == 0 || self.is_split()) {
            self.activate(i, window, cx);
        }
    }

    // ───────────── palette & prompts ─────────────

    fn open_palette(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let (items, placeholder, initial) = self.palette_items(mode, window, cx);
        let palette = cx.new(|cx| Palette::new(mode, items, placeholder, initial, window, cx));
        if mode == Mode::Themes {
            self.palette_restore_theme = Some(Theme::global(cx).data.name.clone());
            palette.update(cx, |p, cx| p.select_hint("current", cx));
        }
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        self.load_palette_symbols(mode, String::new(), cx);
        cx.notify();
    }

    fn palette_items(&self, mode: Mode, window: &Window, cx: &App) -> (Vec<PaletteItem>, &'static str, &'static str) {
        match mode {
            Mode::Commands => {
                let mut items: Vec<PaletteItem> = cx
                    .global::<Commands>()
                    .list
                    .iter()
                    .map(|c| {
                        let mut item = PaletteItem::new(c.display_title(), PaletteAction::Command(c.action.boxed_clone()));
                        let key = window.keystroke_text_for(c.action.as_ref());
                        if !key.is_empty() && key != format!("{:?}", c.action.name()) && !key.contains("::") {
                            item.hint = Some(pretty_keys(&key));
                        }
                        item
                    })
                    .collect();
                items.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
                (items, "Type a command", ">")
            }
            Mode::Files => {
                let root = self.workspace.root();
                let items = self
                    .workspace
                    .files()
                    .iter()
                    .map(|path| {
                        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
                        let mut item = PaletteItem::new(path.file_name().unwrap_or_default().to_string_lossy(), PaletteAction::OpenFile(path.clone()));
                        item.detail = rel.parent().map(|p| p.display().to_string()).filter(|s| !s.is_empty());
                        item.search = Some(rel.display().to_string());
                        item
                    })
                    .collect();
                (items, "Search files by name (type > for commands, : for line)", "")
            }
            Mode::Themes => {
                let current = Theme::global(cx).data.name.clone();
                let items = self
                    .themes
                    .available()
                    .into_iter()
                    .map(|t| {
                        let mut item = PaletteItem::new(t.name.clone(), PaletteAction::Theme(t.name.clone()));
                        item.detail = t.description.clone().or_else(|| t.user.then(|| "user theme".into()));
                        item.hint = Some(if t.name.eq_ignore_ascii_case(&current) { "current".into() } else { t.kind.clone() });
                        item.swatches = t.swatches.clone();
                        item
                    })
                    .collect();
                (items, "Select a color theme (↑↓ to preview)", "")
            }
            Mode::GoToLine => (Vec::new(), "Type a line number", ":"),
            Mode::Symbols => (Vec::new(), "Go to a symbol in this file", "@"),
            Mode::WorkspaceSymbols => (Vec::new(), "Type the name of a type or member", "#"),
            Mode::Prompt | Mode::Branches | Mode::Pick => (Vec::new(), "Name", ""),
        }
    }

    fn open_prompt(&mut self, kind: PromptKind, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match &kind {
            PromptKind::NewFile(_) => "New file name",
            PromptKind::NewFolder(_) => "New folder name",
            PromptKind::Rename(_) => "New name",
            PromptKind::CustomizeTheme => "Name for your theme (inherits everything from the current theme)",
            PromptKind::CreateBranch => "New branch name (created from the current HEAD and checked out)",
            PromptKind::RenameSymbol { .. } => "New name (Enter to rename everywhere, Escape to cancel)",
        };
        self.prompt = Some(kind);
        let palette = cx.new(|cx| Palette::new(Mode::Prompt, Vec::new(), placeholder, initial, window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.prompt = None;
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |e, _| e.focus(window));
        } else {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    fn on_palette_event(&mut self, palette: &Entity<Palette>, event: &PaletteEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PaletteEvent::SwitchMode(mode) => {
                let (items, _, _) = self.palette_items(*mode, window, cx);
                palette.update(cx, |p, cx| p.set_items(*mode, items, cx));
                self.load_palette_symbols(*mode, String::new(), cx);
            }
            PaletteEvent::Query(query) => self.load_palette_symbols(Mode::WorkspaceSymbols, query.clone(), cx),
            PaletteEvent::Preview(PaletteAction::Theme(name)) => {
                let name = name.clone();
                self.apply_theme(&name, window, cx);
            }
            PaletteEvent::Preview(_) => {}
            PaletteEvent::Dismissed => {
                if let Some(name) = self.palette_restore_theme.take() {
                    self.apply_theme(&name, window, cx);
                }
                self.close_palette(window, cx);
            }
            PaletteEvent::Accept(action) => {
                let action = action.clone();
                self.palette_restore_theme = None;
                let prompt = self.prompt.take();
                self.close_palette(window, cx);
                match action {
                    PaletteAction::Command(a) => window.dispatch_action(a, cx),
                    PaletteAction::OpenFile(path) => self.open_file(path, None, window, cx),
                    PaletteAction::Theme(name) => {
                        self.apply_theme(&name, window, cx);
                        self.settings.set("workbench.theme", json!(name));
                    }
                    PaletteAction::GoToLine(line) => {
                        if let Some(editor) = self.active_editor() {
                            editor.update(cx, |e, cx| e.go_to(Point::new(line, 0), cx));
                        }
                    }
                    PaletteAction::Branch(branch) => self.scm.update(cx, |s, cx| s.checkout(branch, cx)),
                    PaletteAction::GoTo(path, at) => self.open_file(path, Some(at), window, cx),
                    PaletteAction::Prompt(text) => {
                        if let Some(kind) = prompt {
                            self.complete_prompt(kind, text, window, cx);
                        }
                    }
                }
            }
        }
    }

    fn complete_prompt(&mut self, kind: PromptKind, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let result: anyhow::Result<()> = (|| {
            match kind {
                PromptKind::NewFile(dir) => {
                    let path = dir.join(&text);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if !path.exists() {
                        std::fs::write(&path, "")?;
                    }
                    self.explorer.update(cx, |e, cx| e.refresh(cx));
                    self.open_file(path, None, window, cx);
                }
                PromptKind::NewFolder(dir) => {
                    std::fs::create_dir_all(dir.join(&text))?;
                    self.explorer.update(cx, |e, cx| e.refresh(cx));
                }
                PromptKind::Rename(path) => {
                    let target = path.parent().map(|p| p.join(&text)).unwrap_or_else(|| PathBuf::from(&text));
                    if target != path {
                        self.move_path(&path, &target, cx)?;
                    }
                }
                PromptKind::RenameSymbol { editor, at, old } => {
                    if text != old
                        && let Some(editor) = editor.upgrade()
                    {
                        editor.update(cx, |e, cx| e.rename_symbol(at, text.clone(), cx));
                    }
                }
                PromptKind::CreateBranch => {
                    let name = text.replace(' ', "-");
                    self.scm.update(cx, |s, cx| s.create_branch(name, cx));
                }
                PromptKind::CustomizeTheme => {
                    let current = Theme::global(cx).data.clone();
                    let file = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>() + ".json";
                    let path = self.paths.user_themes().join(file);
                    if !path.exists() {
                        let body = format!(
                            "{{\n  // Saved changes apply instantly. Every key of the inherited theme can be overridden here.\n  // See themes/byteforge-dark.json next to ByteForge for the full list of keys.\n  \"name\": \"{text}\",\n  \"type\": \"{}\",\n  \"inherits\": \"{}\",\n  \"colors\": {{\n    \"accent\": \"#0078d4\"\n  }},\n  \"tokenColors\": {{\n  }},\n  \"fonts\": {{\n  }},\n  \"metrics\": {{\n  }},\n  \"editor\": {{\n  }},\n  \"layout\": {{\n  }}\n}}\n",
                            current.kind, current.name
                        );
                        std::fs::write(&path, body)?;
                    }
                    self.settings.set("workbench.theme", json!(text));
                    self.apply_theme(&text, window, cx);
                    self.open_file(path, None, window, cx);
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.log(format!("Operation failed: {e}"));
            self.show_panel(PanelView::Output, cx);
        }
    }

    /// Renames or moves a file or folder on disk (never over an existing one) and retargets the open tabs
    /// of it or of anything below it.
    fn move_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) -> anyhow::Result<()> {
        // A case-only rename on a case-insensitive file system reports the target as existing.
        let same_entry = from.parent() == to.parent() && from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
        if to.exists() && !same_entry {
            anyhow::bail!("'{}' already exists", to.display());
        }
        std::fs::rename(from, to)?;
        for tab in &mut self.tabs {
            let Some(rest) = tab.path.as_deref().and_then(|p| p.strip_prefix(from).ok()) else { continue };
            let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            tab.title = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            tab.path = Some(new.clone());
            tab.editor.update(cx, |e, cx| e.set_path(new, cx));
        }
        self.explorer.update(cx, |e, cx| e.path_moved(from, to, cx));
        cx.notify();
        Ok(())
    }

    fn on_explorer_event(&mut self, _: &Entity<Explorer>, event: &ExplorerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ExplorerEvent::Open(path, focus_editor) => {
                self.open_file(path.clone(), None, window, cx);
                if !focus_editor {
                    // Single click: keep the keyboard in the explorer (F2, …) like VS Code; double click goes to the editor.
                    let focus = self.explorer.focus_handle(cx);
                    window.focus(&focus);
                }
            }
            ExplorerEvent::Move { from, to_dir } => {
                let Some(name) = from.file_name() else { return };
                let target = to_dir.join(name);
                if target == *from {
                    return;
                }
                let result = if to_dir.starts_with(from) {
                    Err(anyhow::anyhow!("cannot move a folder into itself"))
                } else {
                    self.move_path(from, &target, cx)
                };
                if let Err(e) = result {
                    self.log(format!("Could not move {} to {}: {e}", from.display(), to_dir.display()));
                    self.show_panel(PanelView::Output, cx);
                }
            }
            ExplorerEvent::NewFile(dir) => self.open_prompt(PromptKind::NewFile(dir.clone()), "", window, cx),
            ExplorerEvent::NewFolder(dir) => self.open_prompt(PromptKind::NewFolder(dir.clone()), "", window, cx),
            ExplorerEvent::Rename(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.open_prompt(PromptKind::Rename(path.clone()), &name, window, cx);
            }
            ExplorerEvent::Delete(path) => {
                let path = path.clone();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let rx = window.prompt(PromptLevel::Warning, &format!("Delete '{name}' permanently?"), None, &["Delete", "Cancel"], cx);
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = rx.await {
                        let result = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                        let _ = this.update(cx, |wb, cx| {
                            if let Err(e) = result {
                                wb.log(format!("Could not delete {}: {e}", path.display()));
                            }
                            wb.explorer.update(cx, |e, cx| e.refresh(cx));
                        });
                    }
                })
                .detach();
            }
            ExplorerEvent::OpenFolderRequested => window.dispatch_action(Box::new(OpenFolder), cx),
        }
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Folder".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |wb, window, cx| wb.open_folder(normalize_path(&path), window, cx));
            }
        })
        .detach();
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    for path in paths {
                        wb.open_file(path, None, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    // ───────────── git ─────────────

    fn on_scm_event(&mut self, scm: &Entity<ScmView>, event: &ScmEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ScmEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            ScmEvent::OpenDiff(change, staged) => self.open_diff(change.clone(), *staged, window, cx),
            ScmEvent::OpenCommitDiff(commit, file) => self.open_commit_diff(commit.clone(), file.clone(), window, cx),
            ScmEvent::Log(text) => self.log(text),
            ScmEvent::StatusChanged => {
                let s = scm.read(cx);
                let decorations = match s.git() {
                    Some(git) => crate::scm::decorations(s.status(), git.root()),
                    None => HashMap::new(),
                };
                let head = Some(s.status().head.clone());
                self.explorer.update(cx, |e, cx| e.set_decorations(decorations, cx));
                // Pull, checkout and discard rewrite files: show the new content in unmodified editors.
                self.reload_changed_tabs(cx);
                // Bases change with HEAD, and when files become tracked or untracked.
                self.git_head = head;
                self.update_git_bases(cx);
                cx.notify();
            }
        }
    }

    /// Loads the HEAD version of an open file for the gutter change markers (off the UI thread).
    fn load_git_base(&self, editor: Entity<EditorView>, path: PathBuf, cx: &mut Context<Self>) {
        let scm = self.scm.read(cx);
        let Some(git) = scm.git().cloned() else {
            editor.update(cx, |e, cx| e.set_git_base(None, cx));
            return;
        };
        if !path.starts_with(git.root()) {
            return;
        }
        let new_file = scm.status().change_for(&path).is_some_and(|c| c.untracked || c.index == 'A');
        let editor = editor.downgrade();
        cx.spawn(async move |_, cx| {
            let base = cx.background_executor().spawn(async move { if new_file { Some(String::new()) } else { git.head_content(&path) } }).await;
            let _ = editor.update(cx, |e, cx| e.set_git_base(base, cx));
        })
        .detach();
    }

    fn update_git_bases(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(path) = &tab.path
                && !tab.editor.read(cx).read_only
            {
                self.load_git_base(tab.editor.clone(), path.clone(), cx);
            }
        }
    }

    /// Reloads unmodified editors whose file changed on disk.
    fn reload_changed_tabs(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let Some(path) = &tab.path else { continue };
            let e = tab.editor.read(cx);
            if e.is_modified() || e.read_only {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else { continue };
            let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
            let text = String::from_utf8_lossy(body);
            if text != e.save_path_text() {
                tab.editor.update(cx, |e, cx| e.reload_text(&text, cx));
            }
        }
    }

    /// Opens a read-only unified diff of a change in a tab (reused when it is already open).
    fn open_diff(&mut self, change: bf_core::git::FileChange, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = change.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{name} ({})", if staged { "Staged" } else if change.untracked { "Untracked" } else { "Working Tree" });
        let rel = change.path.strip_prefix(git.root()).unwrap_or(&change.path).display().to_string().replace('\\', "/");
        let task = cx.background_spawn(async move {
            let (old, new) = git.diff_texts(&change, staged);
            let (a, b) = if staged { (format!("HEAD:{rel}"), format!("index:{rel}")) } else { (format!("index:{rel}"), rel.clone()) };
            bf_core::git::unified_diff(&old, &new, &a, &b)
        });
        self.show_diff_tab(title, task, window, cx);
    }

    /// Opens what a commit changed in a file (first parent ↔ commit) as a read-only diff tab.
    fn open_commit_diff(&mut self, commit: bf_core::git::Commit, file: bf_core::git::CommitFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{name} ({})", commit.short());
        let task = cx.background_spawn(async move {
            let (old, new) = git.commit_diff_texts(&commit, &file);
            let rel = git.relative(&file.path);
            let orig = file.orig.as_deref().map_or_else(|| rel.clone(), |o| git.relative(o));
            let a = match commit.parents.first() {
                Some(p) if file.status != 'A' => format!("{}:{orig}", &p[..p.len().min(7)]),
                _ => "/dev/null".to_owned(),
            };
            let b = if file.status == 'D' { "/dev/null".to_owned() } else { format!("{}:{rel}", commit.short()) };
            bf_core::git::unified_diff(&old, &new, &a, &b)
        });
        self.show_diff_tab(title, task, window, cx);
    }

    /// Shows a unified diff in a read-only tab titled `title`, reusing an open tab of the same title.
    fn show_diff_tab(&mut self, title: String, task: Task<String>, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let diff = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let text = if diff.is_empty() { "No textual changes (binary file or only mode changes).".to_owned() } else { diff };
                let backgrounds: Vec<(usize, &'static str)> = text
                    .lines()
                    .enumerate()
                    .filter_map(|(i, line)| {
                        let key = if line.starts_with("+++") || line.starts_with("---") {
                            return None;
                        } else if line.starts_with('+') {
                            "diffEditor.insertedLineBackground"
                        } else if line.starts_with('-') {
                            "diffEditor.removedLineBackground"
                        } else if line.starts_with("@@") {
                            "diffEditor.hunkHeaderBackground"
                        } else {
                            return None;
                        };
                        Some((i, key))
                    })
                    .collect();
                if let Some(ix) = wb.tabs.iter().position(|t| t.path.is_none() && t.title == title && t.editor.read(cx).read_only) {
                    wb.tabs[ix].editor.update(cx, |e, cx| {
                        e.reload_text(&text, cx);
                        e.line_backgrounds = backgrounds;
                    });
                    wb.activate(ix, window, cx);
                    return;
                }
                let editor = cx.new(|cx| {
                    let mut e = EditorView::new(&text, None, cx);
                    e.read_only = true;
                    e.show_indent_guides = false;
                    e.line_backgrounds = backgrounds;
                    e
                });
                wb.add_tab(editor, None, title, false, window, cx);
            });
        })
        .detach();
    }

    fn open_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let task = cx.background_spawn(async move { git.branches() });
        cx.spawn_in(window, async move |this, cx| {
            let branches = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let branches = match branches {
                    Ok(b) => b,
                    Err(e) => {
                        wb.log(format!("Could not list branches: {e}"));
                        return;
                    }
                };
                let mut items = vec![{
                    let mut item = PaletteItem::new("+ Create new branch…", PaletteAction::Command(Box::new(GitCreateBranch)));
                    item.hint = Some("from HEAD".into());
                    item
                }];
                items.extend(branches.into_iter().map(|b| {
                    let mut item = PaletteItem::new(b.name.clone(), PaletteAction::Branch(b.clone()));
                    item.hint = Some(if b.current { "current".into() } else if b.remote { "remote branch".into() } else { "branch".into() });
                    item
                }));
                wb.menu_open = None;
                let palette = cx.new(|cx| Palette::new(Mode::Branches, items, "Select a branch to check out", "", window, cx));
                let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
                palette.update(cx, |p, cx| p.focus_input(window, cx));
                wb.palette = Some((palette, sub));
                cx.notify();
            });
        })
        .detach();
    }

    fn problems(&self, cx: &App) -> Vec<(PathBuf, crate::editor::Diagnostic)> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            if let Some(path) = &tab.path {
                for d in &tab.editor.read(cx).diagnostics {
                    out.push((path.clone(), d.clone()));
                }
            }
        }
        // Build diagnostics of files that aren't open (open ones show them as editor diagnostics). C# files
        // are left to the language server when it runs.
        let lsp = self.lsp.is_some();
        for (path, d) in &self.build_problems {
            let open = self.tabs.iter().any(|t| t.path.as_ref() == Some(path));
            let csharp = path.extension().is_some_and(|e| e == "cs");
            if !open && !(lsp && csharp) {
                out.push((path.clone(), d.clone()));
            }
        }
        out.sort_by_key(|(p, d)| (severity_rank(d.severity), p.clone(), d.start));
        out
    }
}

fn severity_rank(s: bf_lsp::Severity) -> u8 {
    match s {
        bf_lsp::Severity::Error => 0,
        bf_lsp::Severity::Warning => 1,
        bf_lsp::Severity::Info => 2,
        bf_lsp::Severity::Hint => 3,
    }
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_owned() } else { format!("{}…", text.chars().take(max).collect::<String>()) }
}

// ───────────── rendering ─────────────

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let title = match self.active.and_then(|i| self.tabs.get(i)) {
            Some(tab) => {
                let dirty = if tab.editor.read(cx).is_modified() { "● " } else { "" };
                format!("{dirty}{} — ByteForge", tab.title)
            }
            None => "ByteForge".into(),
        };
        window.set_window_title(&title);
        self.sync_module_view_visibility();
        let tests_visible = self.side_visible && self.side == SideView::Testing;
        self.tests.update(cx, |t, cx| t.set_visible(tests_visible, cx));
        let solution_visible = self.side_visible && self.side == SideView::Solution;
        self.solution.update(cx, |s, cx| s.set_visible(solution_visible, cx));

        // Every region is placed by the theme layout (see layout.rs).
        let lay = &t.layout;
        // Focus is tracked below the title bar: focusing on mouse-down marks the click as handled, which would
        // stop Windows from starting a window drag on the title bar.
        let mut middle = div().track_focus(&self.focus).flex().flex_row().flex_1().min_h_0().min_w_0().overflow_hidden();
        let mut has_editor = false;
        for part in &lay.main {
            match part.as_str() {
                "activityBar" => middle = middle.child(self.render_activity_bar(lay.right_of_editor("activityBar"), false, cx)),
                "sideBar" if self.side_visible => middle = middle.child(self.render_side_bar(lay.right_of_editor("sideBar"), cx)),
                "editor" if !has_editor => {
                    has_editor = true;
                    middle = middle.child(self.render_main(window, cx));
                }
                "panel" if self.panel_visible => {
                    let place = if lay.right_of_editor("panel") { PanelPlace::Right } else { PanelPlace::Left };
                    middle = middle.child(self.render_panel(place, cx));
                }
                _ => {}
            }
        }
        if !has_editor {
            middle = middle.child(self.render_main(window, cx));
        }
        let mut middle = Some(middle);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        // A custom title bar carries the window buttons and the drag area, so it can move but not disappear.
        if self.custom_titlebar && !lay.workbench.iter().any(|p| p == "titleBar") {
            rows.push(self.render_title_bar(window, cx).into_any_element());
        }
        for part in &lay.workbench {
            match part.as_str() {
                "titleBar" => rows.push(self.render_title_bar(window, cx).into_any_element()),
                "statusBar" => rows.push(self.render_status_bar(cx).into_any_element()),
                "main" => rows.extend(middle.take().map(|m| m.into_any_element())),
                _ => {}
            }
        }
        rows.extend(middle.take().map(|m| m.into_any_element()));

        let entity = cx.entity();
        let root = div()
            .id("workbench")
            .key_context("Workbench")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(t.bg("window.background"))
            .text_color(t.c("window.foreground"))
            .font_family(t.ui_font.clone())
            .text_size(t.m("ui.size", 13.0))
            .font_weight(t.weight("ui.weight", gpui::FontWeight::NORMAL))
            .on_mouse_move(cx.listener(|wb, e: &MouseMoveEvent, _, cx| wb.on_sash_drag(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|wb, _, _, cx| {
                if wb.sash.take().is_some() {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|wb, _: &NewFile, w, cx| wb.new_file(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenFile, w, cx| wb.pick_files(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenFolder, w, cx| wb.pick_folder(w, cx)))
            .on_action(cx.listener(|wb, _: &Save, w, cx| {
                if let Some(i) = wb.active {
                    wb.save_formatted(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &FormatDocument, w, cx| {
                if let Some(i) = wb.active
                    && !wb.format_tab(i, None, false, w, cx)
                {
                    let name = wb.tabs[i].title.clone();
                    wb.log(format!("No formatter handles {name}. CSharpier formats C# and MSBuild/XML files; csharp-ls formats C#."));
                    wb.show_panel(PanelView::Output, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &FormatDocumentWith, w, cx| wb.open_formatter_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &UseFormatter, w, cx| {
                wb.settings.set("editor.defaultFormatter", json!(action.id));
                if let Some(i) = wb.active {
                    wb.format_tab(i, Some(action.id.clone()), false, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &SaveAs, w, cx| {
                if let Some(i) = wb.active {
                    wb.save_as(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &SaveAll, w, cx| wb.save_all(w, cx)))
            .on_action(cx.listener(|wb, _: &CloseEditor, w, cx| {
                if let Some(i) = wb.active {
                    wb.close_tab(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &CloseAllEditors, w, cx| {
                for i in (0..wb.tabs.len()).rev() {
                    wb.close_tab(i, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &Quit, w, cx| {
                if wb.should_close(w, cx) {
                    cx.quit();
                }
            }))
            .on_action(cx.listener(|wb, _: &CommandPalette, w, cx| wb.open_palette(Mode::Commands, w, cx)))
            .on_action(cx.listener(|wb, _: &QuickOpen, w, cx| wb.open_palette(Mode::Files, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToLine, w, cx| wb.open_palette(Mode::GoToLine, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToSymbolInEditor, w, cx| wb.open_palette(Mode::Symbols, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToSymbolInWorkspace, w, cx| wb.open_palette(Mode::WorkspaceSymbols, w, cx)))
            .on_action(cx.listener(|wb, _: &SelectTheme, w, cx| wb.open_palette(Mode::Themes, w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleSideBar, _, cx| {
                wb.side_visible = !wb.side_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &TogglePanel, _, cx| {
                wb.panel_visible = !wb.panel_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &ShowExplorer, w, cx| wb.toggle_side(SideView::Explorer, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSearch, w, cx| wb.toggle_side(SideView::Search, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowModules, w, cx| wb.toggle_side(SideView::Modules, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSourceControl, w, cx| wb.toggle_side(SideView::SourceControl, w, cx)))
            .on_action(cx.listener(|wb, _: &GitRefresh, _, cx| wb.scm.update(cx, |s, cx| s.refresh(cx))))
            .on_action(cx.listener(|wb, _: &GitCommit, w, cx| {
                wb.show_side(SideView::SourceControl, cx);
                wb.scm.update(cx, |s, cx| s.commit(w, cx))
            }))
            .on_action(cx.listener(|wb, _: &GitStageAll, _, cx| wb.scm.update(cx, |s, cx| s.stage_all(cx))))
            .on_action(cx.listener(|wb, _: &GitUnstageAll, _, cx| wb.scm.update(cx, |s, cx| s.unstage_all(cx))))
            .on_action(cx.listener(|wb, _: &GitPull, _, cx| wb.scm.update(cx, |s, cx| s.pull(cx))))
            .on_action(cx.listener(|wb, _: &GitPush, _, cx| wb.scm.update(cx, |s, cx| s.push(cx))))
            .on_action(cx.listener(|wb, _: &GitFetch, _, cx| wb.scm.update(cx, |s, cx| s.fetch(cx))))
            .on_action(cx.listener(|wb, _: &GitSync, _, cx| wb.scm.update(cx, |s, cx| s.sync(cx))))
            .on_action(cx.listener(|wb, _: &GitCheckout, w, cx| wb.open_branch_picker(w, cx)))
            .on_action(cx.listener(|wb, _: &GitCreateBranch, w, cx| wb.open_prompt(PromptKind::CreateBranch, "", w, cx)))
            .on_action(cx.listener(|wb, _: &GitInit, _, cx| {
                wb.show_side(SideView::SourceControl, cx);
                wb.scm.update(cx, |s, cx| s.init(cx))
            }))
            .on_action(cx.listener(|wb, _: &GitOpenChanges, w, cx| {
                let path = wb.active.and_then(|i| wb.tabs.get(i)).and_then(|t| t.path.clone());
                let change = path.and_then(|p| wb.scm.read(cx).status().change_for(&p).cloned());
                match change {
                    Some(c) => {
                        let staged = !c.is_unstaged();
                        wb.open_diff(c, staged, w, cx)
                    }
                    None => wb.log("The current file has no changes."),
                }
            }))
            .on_action(cx.listener(|wb, _: &ShowProblems, _, cx| wb.show_panel(PanelView::Problems, cx)))
            .on_action(cx.listener(|wb, _: &ShowOutput, _, cx| wb.show_panel(PanelView::Output, cx)))
            .on_action(cx.listener(|wb, _: &NextEditor, w, cx| wb.cycle_tab(1, w, cx)))
            .on_action(cx.listener(|wb, _: &SplitEditor, w, cx| wb.split_editor(w, cx)))
            .on_action(cx.listener(|wb, _: &MarkdownOpenPreview, w, cx| wb.open_markdown_preview(false, w, cx)))
            .on_action(cx.listener(|wb, _: &MarkdownOpenPreviewToSide, w, cx| wb.open_markdown_preview(true, w, cx)))
            .on_action(cx.listener(|wb, _: &FocusFirstGroup, w, cx| wb.focus_group(0, w, cx)))
            .on_action(cx.listener(|wb, _: &FocusSecondGroup, w, cx| wb.focus_group(1, w, cx)))
            .on_action(cx.listener(|wb, _: &PreviousEditor, w, cx| wb.cycle_tab(-1, w, cx)))
            .on_action(cx.listener(|wb, _: &CustomizeTheme, w, cx| {
                let name = format!("{} Custom", Theme::global(cx).data.name);
                wb.open_prompt(PromptKind::CustomizeTheme, &name, w, cx)
            }))
            .on_action(cx.listener(|wb, _: &EditTheme, w, cx| {
                if let Some(path) = Theme::global(cx).data.file_path.clone() {
                    wb.open_file(path, None, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &ReloadTheme, w, cx| wb.reload_theme(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenSettings, w, cx| {
                let path = wb.settings.path().to_path_buf();
                wb.open_file(path, None, w, cx)
            }))
            .on_action(cx.listener(|wb, _: &About, _, cx| {
                wb.log(format!("ByteForge {} — Rust + GPUI, {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
                wb.log(format!("Config: {}", wb.paths.config_dir.display()));
                wb.show_panel(PanelView::Output, cx);
            }))
            .on_action(cx.listener(|wb, _: &Build, w, cx| wb.run_dotnet("build", w, cx)))
            .on_action(cx.listener(|wb, _: &Run, w, cx| wb.run_dotnet("run", w, cx)))
            .on_action(cx.listener(|wb, _: &Test, w, cx| wb.run_all_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &ShowTesting, w, cx| wb.toggle_side(SideView::Testing, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSolution, w, cx| wb.toggle_side(SideView::Solution, w, cx)))
            .on_action(cx.listener(|wb, _: &RunAllTests, w, cx| wb.run_all_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &RerunFailedTests, w, cx| wb.rerun_failed_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &RunTestAtCursor, w, cx| wb.run_test_at_cursor(false, w, cx)))
            .on_action(cx.listener(|wb, _: &DebugTestAtCursor, w, cx| wb.run_test_at_cursor(true, w, cx)))
            .on_action(cx.listener(|wb, a: &ModulePickChoice, _, _| wb.module_pick_choice(a)))
            .on_action(cx.listener(|wb, _: &RefreshTests, _, cx| wb.tests.update(cx, |t, cx| t.discover(cx))))
            .on_action(cx.listener(|wb, _: &Clean, w, cx| wb.run_dotnet("clean", w, cx)))
            .on_action(cx.listener(|wb, _: &Restore, w, cx| wb.run_dotnet("restore", w, cx)))
            .on_action(cx.listener(|wb, _: &Stop, _, cx| {
                if wb.debug.is_some() {
                    wb.debug_stop(cx);
                } else if let Some(d) = &wb.dotnet {
                    d.kill();
                    wb.log("Process stopped.");
                }
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &StartDebugging, w, cx| wb.debug_start(w, cx)))
            .on_action(cx.listener(|wb, _: &RestartDebugging, w, cx| wb.debug_restart(w, cx)))
            .on_action(cx.listener(|wb, _: &Pause, _, cx| wb.debug_pause(cx)))
            .on_action(cx.listener(|wb, _: &StepOver, _, cx| wb.debug_step(debug::Step::Over, cx)))
            .on_action(cx.listener(|wb, _: &StepInto, _, cx| wb.debug_step(debug::Step::Into, cx)))
            .on_action(cx.listener(|wb, _: &StepOut, _, cx| wb.debug_step(debug::Step::Out, cx)))
            .on_action(cx.listener(|wb, _: &ToggleBreakpoint, _, cx| {
                if let Some(editor) = wb.active_editor() {
                    editor.update(cx, |e, cx| {
                        if e.glyph_margin {
                            let line = e.state.caret().line;
                            e.toggle_breakpoint(line, cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|wb, _: &RemoveAllBreakpoints, _, cx| wb.remove_all_breakpoints(cx)))
            .on_action(cx.listener(|wb, _: &AttachToProcess, w, cx| wb.open_attach_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &AttachToPid, w, cx| wb.debug_attach(action.pid, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowDebug, w, cx| wb.toggle_side(SideView::Debug, w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleTerminal, w, cx| wb.toggle_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &NewTerminal, w, cx| wb.new_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &KillTerminal, w, cx| wb.kill_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &SelectDefaultShell, w, cx| wb.open_shell_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &SetDefaultShell, w, cx| {
                wb.settings.set("terminal.shell", json!(action.shell));
                wb.new_terminal(w, cx);
            }))
            .on_action(cx.listener(|wb, _: &ShowDebugConsole, w, cx| {
                wb.show_panel(PanelView::DebugConsole, cx);
                let focus = wb.debug_input.focus_handle(cx);
                w.focus(&focus);
            }))
            .on_action(cx.listener(|wb, _: &ToggleLineEnding, _, cx| {
                if let Some(editor) = wb.active_editor() {
                    editor.update(cx, |e, cx| {
                        let next = match e.state.buffer.line_ending() {
                            bf_text::LineEnding::Lf => bf_text::LineEnding::CrLf,
                            bf_text::LineEnding::CrLf => bf_text::LineEnding::Lf,
                        };
                        e.state.buffer.set_line_ending(next);
                        cx.notify();
                    });
                }
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &ReloadCSharp, _, cx| wb.start_lsp(cx)))
            .on_action(cx.listener(|wb, action: &RunModuleCommand, _, cx| {
                if let Some(module) = wb.plugin_commands.get(&action.id)
                    && let Some(p) = wb.plugins.iter().find(|p| &p.manifest.id == module)
                {
                    p.execute(&action.id);
                }
                cx.notify();
            }))
            .children(rows);

        let _ = entity;
        let root = match &self.palette {
            Some((palette, _)) => root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.bg("overlay.background"))
                    .flex()
                    .justify_center()
                    .pt(t.edges("palette.margin", 48.0).top)
                    .on_mouse_down(MouseButton::Left, cx.listener(|wb, _, w, cx| {
                        if let Some(name) = wb.palette_restore_theme.take() {
                            wb.apply_theme(&name, w, cx);
                        }
                        wb.close_palette(w, cx)
                    }))
                    .child(div().child(palette.clone())),
            ),
            None => root,
        };
        root.children(self.render_resize_edges(window))
    }
}

impl Workbench {
    fn on_sash_drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(sash) = self.sash else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.sash = None;
            return;
        }
        match sash {
            Sash::Side { start_x, start_w, right } => {
                let dx = f32::from(e.position.x) - start_x;
                self.side_width = (if right { start_w - dx } else { start_w + dx }).clamp(140.0, 900.0);
            }
            Sash::Panel { start_y, start_h, top } => {
                let dy = f32::from(e.position.y) - start_y;
                self.panel_height = (if top { start_h + dy } else { start_h - dy }).clamp(80.0, 1200.0);
            }
            Sash::PanelSide { start_x, start_w, right } => {
                let dx = f32::from(e.position.x) - start_x;
                self.panel_width = (if right { start_w - dx } else { start_w + dx }).clamp(160.0, 1400.0);
            }
        }
        cx.notify();
    }

    fn render_resize_edges(&self, window: &Window) -> Vec<gpui::AnyElement> {
        // Client-side decorations (Linux): resize from the window edges.
        if !matches!(window.window_decorations(), gpui::Decorations::Client { .. }) || window.is_maximized() {
            return Vec::new();
        }
        let edge = |id: &'static str, e: gpui::ResizeEdge, cursor: gpui::CursorStyle| {
            div().id(id).absolute().cursor(cursor).on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(e);
            })
        };
        let g = px(5.);
        vec![
            edge("re-t", gpui::ResizeEdge::Top, gpui::CursorStyle::ResizeUpDown).top_0().left(g).right(g).h(g).into_any_element(),
            edge("re-b", gpui::ResizeEdge::Bottom, gpui::CursorStyle::ResizeUpDown).bottom_0().left(g).right(g).h(g).into_any_element(),
            edge("re-l", gpui::ResizeEdge::Left, gpui::CursorStyle::ResizeLeftRight).left_0().top(g).bottom(g).w(g).into_any_element(),
            edge("re-r", gpui::ResizeEdge::Right, gpui::CursorStyle::ResizeLeftRight).right_0().top(g).bottom(g).w(g).into_any_element(),
            edge("re-tl", gpui::ResizeEdge::TopLeft, gpui::CursorStyle::ResizeUpLeftDownRight).top_0().left_0().size(g).into_any_element(),
            edge("re-tr", gpui::ResizeEdge::TopRight, gpui::CursorStyle::ResizeUpRightDownLeft).top_0().right_0().size(g).into_any_element(),
            edge("re-bl", gpui::ResizeEdge::BottomLeft, gpui::CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().size(g).into_any_element(),
            edge("re-br", gpui::ResizeEdge::BottomRight, gpui::CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().size(g).into_any_element(),
        ]
    }

    fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let custom = self.custom_titlebar;
        let at_bottom = lay.title_bar_at_bottom();
        let left = self.title_items(&lay.title_left, "l", at_bottom, window, cx);
        let center = self.title_items(&lay.title_center, "c", at_bottom, window, cx);
        let mut right = self.title_items(&lay.title_right, "r", at_bottom, window, cx);
        // A custom title bar always keeps its window buttons, even if a theme forgets to place them.
        if custom && !lay.contains_title_item("windowControls") {
            right.extend(self.title_items(&["windowControls".to_owned()], "w", at_bottom, window, cx));
        }
        let border = px(f32::from(t.edges("titleBar.borderThickness", 1.0).bottom));
        div()
            .id("title-bar")
            .relative()
            .flex_none()
            .h(t.m("titleBar.height", 36.0))
            .w_full()
            .flex()
            .items_center()
            .bg(t.bg("titleBar.background"))
            .map(|d| if at_bottom { d.border_t(border) } else { d.border_b(border) })
            .border_color(t.c("titleBar.border"))
            .when(custom, |d| {
                d.window_control_area(WindowControlArea::Drag).on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, _| {
                    // On Windows the title bar is a native caption area: the OS handles drag and double-click.
                    if cfg!(windows) {
                        return;
                    }
                    if e.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                })
            })
            // The center zone is an overlay so it stays centered in the window, whatever the sides contain.
            .when(!center.is_empty(), |d| d.child(div().absolute().inset_0().flex().items_center().justify_center().gap(t.m("titleBar.spacing", 2.0)).children(center)))
            .child(div().h_full().flex_1().min_w_0().flex().items_center().gap(t.m("titleBar.spacing", 2.0)).children(left))
            .child(div().h_full().flex().items_center().gap(t.m("titleBar.spacing", 2.0)).children(right))
    }

    /// Title bar parts: logo, menu, title, commandCenter, layoutControls (= toggleSideBar, togglePanel,
    /// themePicker), branch, windowControls, spacer, or any command (`command:<id>[:<icon>]`).
    fn title_items(&mut self, names: &[String], zone: &str, upward: bool, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::global(cx);
        let fg = if window.is_window_active() { t.c("titleBar.foreground") } else { t.c("titleBar.inactiveForeground") };
        let hover = t.c("toolbar.hoverBackground");
        let icon_size = t.m("titleBar.iconSize", 16.0);
        let title_icon = |id: SharedString, name: &str, tip: String, active: bool| {
            let color = if active { t.c("titleIcon.activeForeground") } else { fg };
            div()
                .id(id)
                // Title bar controls occlude the drag area beneath them; otherwise Windows reports the point as
                // the caption (the first window-control area found) and swallows the click to move the window.
                .occlude()
                .flex_none()
                .p(t.edges("titleIcon.padding", 5.0).top)
                .rounded(t.radius("titleIcon.radius", 4.0))
                .opacity(if active { 1.0 } else { t.n("titleIcon.inactiveOpacity", 0.75) })
                .cursor_pointer()
                .hover(move |s| s.bg(hover).opacity(1.0))
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, color))
        };
        let expanded: Vec<&str> = names
            .iter()
            .flat_map(|n| if n == "layoutControls" { vec!["toggleSideBar", "togglePanel", "themePicker"] } else { vec![n.as_str()] })
            .collect();
        let mut out = Vec::new();
        for (n, name) in expanded.into_iter().enumerate() {
            let id = SharedString::from(format!("tb-{zone}-{n}"));
            match name {
                "logo" => out.push(div().flex_none().pl_3().pr_2().child(icon("logo", t.m("titleBar.logoSize", 16.0), t.c("titleBar.logo"))).into_any_element()),
                "menu" => out.push(self.render_menu_bar(upward, fg, cx).into_any_element()),
                "title" => {
                    let text = match self.active.and_then(|i| self.tabs.get(i)) {
                        Some(tab) => format!("{} — {}", tab.title, self.workspace.root().and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "ByteForge".into())),
                        None => "ByteForge".into(),
                    };
                    out.push(div().px_2().min_w_0().overflow_hidden().whitespace_nowrap().text_size(t.m("titleBar.titleFontSize", 12.0)).text_color(fg).child(text).into_any_element());
                }
                "commandCenter" => {
                    if f32::from(window.viewport_size().width) >= t.n("commandCenter.minWindowWidth", 820.0) {
                        out.push(self.render_command_center(cx));
                    }
                }
                "branch" => {
                    if self.scm.read(cx).git().is_some() {
                        let branch = self.scm.read(cx).status().branch.clone().unwrap_or_else(|| "(detached)".into());
                        out.push(
                            div()
                                .id(id)
                                .occlude()
                                .flex_none()
                                .px_2()
                                .py(px(3.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded(t.radius("titleIcon.radius", 4.0))
                                .text_size(t.m("titleBar.titleFontSize", 12.0))
                                .text_color(fg)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .tooltip(tooltip("Checkout branch…"))
                                .child(icon("sourceControl", icon_size, fg))
                                .child(branch)
                                .on_click(cx.listener(|wb, _, w, cx| wb.open_branch_picker(w, cx)))
                                .into_any_element(),
                        );
                    }
                }
                "toggleSideBar" => {
                    let icon_name = if t.layout.right_of_editor("sideBar") { "layoutSideBarRight" } else { "layoutSideBar" };
                    out.push(title_icon(id, icon_name, "Toggle Side Bar (Ctrl+B)".into(), self.side_visible).on_click(cx.listener(|wb, _, _, cx| {
                        wb.side_visible = !wb.side_visible;
                        cx.notify();
                    })).into_any_element());
                }
                "togglePanel" => out.push(title_icon(id, "layoutPanel", "Toggle Panel (Ctrl+J)".into(), self.panel_visible).on_click(cx.listener(|wb, _, _, cx| {
                    wb.panel_visible = !wb.panel_visible;
                    cx.notify();
                })).into_any_element()),
                "themePicker" => out.push(title_icon(id, "palette", "Color Theme (Ctrl+K)".into(), false).on_click(cx.listener(|wb, _, w, cx| wb.open_palette(Mode::Themes, w, cx))).into_any_element()),
                "windowControls" => {
                    if self.custom_titlebar && !cfg!(target_os = "macos") {
                        out.push(render_window_controls(window, cx).into_any_element());
                    }
                }
                "spacer" => out.push(div().flex_1().into_any_element()),
                other => {
                    if let Some((action, title, icon_name)) = command_button(other, cx) {
                        out.push(title_icon(id, &icon_name, title, false).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).into_any_element());
                    }
                }
            }
        }
        out
    }

    fn render_command_center(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let root_name = self.workspace.root().and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "ByteForge".into());
        let cc_hover_bg = t.c("commandCenter.hoverBackground");
        let cc_hover_border = t.c("commandCenter.hoverBorder");
        div()
            .id("command-center")
            .occlude()
            .flex_shrink()
            .min_w(px(120.))
            .w(t.m("commandCenter.width", 380.0))
            .h(t.m("commandCenter.height", 24.0))
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(t.bg("commandCenter.background"))
            .border(px(f32::from(t.edges("commandCenter.borderThickness", 1.0).top)))
            .border_color(t.c("commandCenter.border"))
            .rounded(t.radius("commandCenter.radius", 6.0))
            .text_color(t.c("commandCenter.foreground"))
            .text_size(t.m("commandCenter.fontSize", 12.0))
            .cursor_pointer()
            .hover(move |s| s.bg(cc_hover_bg).border_color(cc_hover_border))
            .tooltip(tooltip("Search files by name (Ctrl+P)"))
            .child(icon("search", t.m("commandCenter.iconSize", 13.0), t.c("commandCenter.foreground")))
            .child(root_name)
            .on_click(cx.listener(|wb, _, w, cx| {
                let mode = if wb.workspace.root().is_some() { Mode::Files } else { Mode::Commands };
                wb.open_palette(mode, w, cx)
            }))
            .into_any_element()
    }

    /// The menu bar; `layout."menuBar.menus"` picks and orders the menus.
    fn render_menu_bar(&mut self, upward: bool, fg: gpui::Hsla, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let hover = t.c("toolbar.hoverBackground");
        let mut row = div().flex_none().flex().items_center();
        for name in &t.layout.menus {
            let Some(i) = MENUS.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) else { continue };
            let open = self.menu_open == Some(i);
            let dropdown = open.then(|| self.render_menu_dropdown(i, upward, cx));
            row = row.child(
                div()
                    .id(SharedString::from(format!("menu-{name}")))
                    .occlude()
                    .relative()
                    .px(t.edges("menuBarItem.padding", 8.0).left)
                    .py(t.edges("menuBarItem.padding", 4.0).top)
                    .rounded(t.radius("menuBarItem.radius", 4.0))
                    .text_size(t.m("menuBar.fontSize", 13.0))
                    .text_color(fg)
                    .cursor_pointer()
                    .when(open, |d| d.bg(hover))
                    .hover(move |s| s.bg(hover))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |wb, _, _, cx| {
                        wb.menu_open = if wb.menu_open == Some(i) { None } else { Some(i) };
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_hover(cx.listener(move |wb, hovered: &bool, _, cx| {
                        if *hovered && wb.menu_open.is_some() && wb.menu_open != Some(i) {
                            wb.menu_open = Some(i);
                            cx.notify();
                        }
                    }))
                    .child(MENUS[i].0)
                    .children(dropdown),
            );
        }
        row
    }

    /// Dropdown anchored to its menu button (opens upwards when the title bar is at the bottom).
    fn render_menu_dropdown(&self, i: usize, upward: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let (_, entries) = MENUS[i];
        let commands = cx.global::<Commands>();
        let items: Vec<gpui::AnyElement> = entries
            .iter()
            .enumerate()
            .filter_map(|(ix, id)| {
                if *id == "-" {
                    return Some(crate::menu::separator(&t).into_any_element());
                }
                let cmd = commands.get(id)?;
                let action = cmd.action.boxed_clone();
                Some(
                    crate::menu::item(&t, ix, cmd.title.clone(), Some(default_key(id).into()))
                        .on_click(cx.listener(move |wb, _, w, cx| {
                            wb.menu_open = None;
                            w.dispatch_action(action.boxed_clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect();
        let menu = crate::menu::panel(&t, 240.)
            .on_mouse_down_out(cx.listener(|wb, _, _, cx| {
                wb.menu_open = None;
                cx.notify();
            }))
            .children(items);
        let corner = if upward { gpui::Corner::BottomLeft } else { gpui::Corner::TopLeft };
        div()
            .absolute()
            .left_0()
            .map(|d| if upward { d.top_0() } else { d.top(gpui::relative(1.)) })
            .child(gpui::deferred(gpui::anchored().anchor(corner).snap_to_window().child(menu)).with_priority(2))
            .into_any_element()
    }

    /// The activity bar: vertical in the main row, or horizontal when placed in `sideBar.order`.
    /// Items: explorer, search, scm, modules, settings, spacer, or any command (`command:<id>[:<icon>]`).
    fn render_activity_bar(&mut self, right: bool, horizontal: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let (item_w, item_h, icon_size) = if horizontal {
            (t.m("activityBar.horizontalItemWidth", 40.0), t.m("activityBar.horizontalHeight", 36.0), t.m("activityBar.horizontalIconSize", 18.0))
        } else {
            (t.m("activityBar.width", 48.0), t.m("activityBar.itemHeight", 48.0), t.m("activityBar.iconSize", 24.0))
        };
        let indicator = if horizontal {
            if t.data.metrics.contains_key("activityBar.indicatorBottom.border") {
                t.edges("activityBar.indicatorBottom.border", 0.0)
            } else {
                gpui::Edges { bottom: px(2.), ..Default::default() }
            }
        } else if right {
            t.edges("activityBar.indicatorRight.border", 0.0)
        } else {
            t.edges("activityBar.indicatorLeft.border", 2.0)
        };
        let active_bg = t.bg("activityBar.activeBackground");
        let hover_bg = t.c("activityBar.hoverBackground");
        let badge_size = t.n("activityBarBadge.size", 16.0);
        let badge_bg = t.bg("activityBarBadge.background");
        let badge_fg = t.c("activityBarBadge.foreground");
        let badge = move |count: usize| {
            (count > 0).then(|| {
                div()
                    .absolute()
                    .bottom(px(f32::from(item_h) / 2.0 - f32::from(icon_size) / 2.0 - badge_size / 3.0))
                    .right(px(f32::from(item_w) / 2.0 - f32::from(icon_size) / 2.0 - badge_size / 2.0))
                    .min_w(px(badge_size))
                    .h(px(badge_size))
                    .px(px(3.))
                    .rounded(px(badge_size / 2.0))
                    .bg(badge_bg)
                    .text_color(badge_fg)
                    .text_size(px(badge_size * 0.62))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if count > 999 { "999+".to_owned() } else { count.to_string() })
            })
        };
        // Optional text under each icon (vertical bar only) and a horizontal inset that turns full-width items
        // into rounded tiles.
        let label_size = if horizontal { 0.0 } else { t.n("activityBar.labelSize", 0.0) };
        let inset = if horizontal { px(0.) } else { t.m("activityBar.itemInset", 0.0) };
        let active_icon = if t.has("activityBar.activeIconForeground") { t.c("activityBar.activeIconForeground") } else { t.c("activityBar.foreground") };
        let item = |id: SharedString, name: &str, tip: String, label: &str, active: bool| {
            let color = if active { t.c("activityBar.foreground") } else { t.c("activityBar.inactiveForeground") };
            let icon_color = if active { active_icon } else { color };
            let hover_fg = t.c("activityBar.foreground");
            div()
                .id(id)
                .flex_none()
                .w(item_w - inset * 2.0)
                .h(item_h)
                .mx(inset)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .relative()
                .text_color(color)
                .rounded(t.radius("activityBar.itemRadius", 0.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .when(active, |d| {
                    d.bg(active_bg)
                        .border_color(t.c("activityBar.activeBorder"))
                        .border_l(indicator.left)
                        .border_r(indicator.right)
                        .border_t(indicator.top)
                        .border_b(indicator.bottom)
                })
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, icon_color))
                .when(label_size > 0.0 && !label.is_empty(), |d| {
                    d.child(div().text_size(px(label_size)).line_height(px(label_size * 1.2)).font_weight(gpui::FontWeight::MEDIUM).child(label.to_owned()))
                })
        };
        let side_visible = self.side_visible;
        let side = self.side;
        let scm_count = self.scm.read(cx).status().count();
        let mut bar = div()
            .flex_none()
            .flex()
            .gap(t.m("activityBar.spacing", 0.0))
            .bg(t.bg("activityBar.background"))
            .border_color(t.c("activityBar.border"))
            .map(|d| {
                if horizontal {
                    d.w_full().h(item_h).flex_row().px(t.edges("activityBar.horizontalPadding", 4.0).left).border_b_1()
                } else {
                    d.w(item_w).h_full().flex_col().when(right, |d| d.border_l_1()).when(!right, |d| d.border_r_1())
                }
            });
        // Module views: where the layout names them, else before the first spacer (or at the end).
        let module_views: Vec<(String, String, String, u64)> = self.module_views.iter().map(|v| (v.id.clone(), v.title.clone(), v.icon.clone(), v.badge)).collect();
        let module_item = |n: String, (view_id, title, icon_name, count): &(String, String, String, u64), cx: &mut Context<Self>| {
            let active = self.module_view_active(view_id);
            let view_id = view_id.clone();
            item(SharedString::from(format!("act-m-{n}-{}", if horizontal { "h" } else { "v" })), icon_name, title.clone(), title, active)
                .children(badge(*count as usize))
                .on_click(cx.listener(move |wb, _, _, cx| wb.toggle_module_view(&view_id, cx)))
        };
        let mut unplaced: Vec<&(String, String, String, u64)> = module_views.iter().filter(|v| !t.layout.activity_items.iter().any(|i| i == &v.0)).collect();
        for (n, name) in t.layout.activity_items.iter().enumerate() {
            let id = SharedString::from(format!("act-{n}-{}", if horizontal { "h" } else { "v" }));
            if let Some(v) = module_views.iter().find(|v| &v.0 == name) {
                bar = bar.child(module_item(n.to_string(), v, cx));
                continue;
            }
            if name == "spacer" {
                for (k, v) in std::mem::take(&mut unplaced).into_iter().enumerate() {
                    bar = bar.child(module_item(format!("{n}-{k}"), v, cx));
                }
            }
            let view = match name.as_str() {
                "explorer" => Some((SideView::Explorer, "files", "Explorer (Ctrl+Shift+E)", "Files")),
                "search" => Some((SideView::Search, "search", "Search (Ctrl+Shift+F)", "Search")),
                "scm" => Some((SideView::SourceControl, "sourceControl", "Source Control (Ctrl+Shift+G)", "Git")),
                "debug" => Some((SideView::Debug, "debug", "Run and Debug (Ctrl+Shift+D)", "Debug")),
                "testing" | "tests" => Some((SideView::Testing, "beaker", "Testing", "Tests")),
                "solution" => Some((SideView::Solution, "fileSolution", "Solution", "Solution")),
                "modules" => Some((SideView::Modules, "puzzle", "Modules", "Modules")),
                _ => None,
            };
            if let Some((view, icon_name, tip, label)) = view {
                let mut el = item(id, icon_name, tip.into(), label, side_visible && side == view);
                if view == SideView::SourceControl {
                    el = el.children(badge(scm_count));
                }
                bar = bar.child(el.on_click(cx.listener(move |wb, _, w, cx| wb.toggle_side(view, w, cx))));
                continue;
            }
            match name.as_str() {
                "spacer" => bar = bar.child(div().flex_1()),
                "settings" => {
                    bar = bar.child(item(id, "settings", "Settings (Ctrl+,)".into(), "", false).on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(OpenSettings), cx))));
                }
                other => {
                    if let Some((action, title, icon_name)) = command_button(other, cx) {
                        bar = bar.child(item(id, &icon_name, title, "", false).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)));
                    }
                }
            }
        }
        for (k, v) in unplaced.into_iter().enumerate() {
            bar = bar.child(module_item(format!("end-{k}"), v, cx));
        }
        bar
    }

    /// The side bar; `sideBar.order` arranges header, content and (optionally) a horizontal activity bar.
    fn render_side_bar(&mut self, right: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let title: SharedString = match self.side {
            SideView::Explorer => "EXPLORER".into(),
            SideView::Search => "SEARCH".into(),
            SideView::SourceControl => "SOURCE CONTROL".into(),
            SideView::Debug => "RUN AND DEBUG".into(),
            SideView::Testing => "TESTING".into(),
            SideView::Solution => "SOLUTION".into(),
            SideView::Module => self.module_views.iter().find(|v| Some(&v.id) == self.module_view.as_ref()).map_or("MODULE".into(), |v| v.title.to_uppercase().into()),
            SideView::Modules => "MODULES".into(),
        };
        let mut content: Option<gpui::AnyElement> = Some(match self.side {
            SideView::Explorer => self.explorer.clone().into_any_element(),
            SideView::Search => self.search.clone().into_any_element(),
            SideView::SourceControl => self.scm.clone().into_any_element(),
            SideView::Debug => self.render_debug_view(cx),
            SideView::Testing => self.tests.clone().into_any_element(),
            SideView::Solution => self.solution.clone().into_any_element(),
            SideView::Module => self.render_module_view(cx),
            SideView::Modules => self.render_modules(cx).into_any_element(),
        });
        let sash_hover = t.c("sash.hoverBorder");
        let width = self.side_width;
        let sash = div()
            .id("side-sash")
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(5.))
            .when(right, |d| d.left(px(-2.)))
            .when(!right, |d| d.right(px(-2.)))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .hover(move |s| s.bg(sash_hover))
            .on_mouse_down(MouseButton::Left, cx.listener(move |wb, e: &MouseDownEvent, _, cx| {
                wb.sash = Some(Sash::Side { start_x: f32::from(e.position.x), start_w: width, right });
                cx.stop_propagation();
            }));
        let mut col = div()
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.bg("sideBar.background"))
            .text_color(t.c("sideBar.foreground"))
            .border_color(t.c("sideBar.border"))
            .when(right, |d| d.border_l_1())
            .when(!right, |d| d.border_r_1());
        for part in &t.layout.side_bar {
            match part.as_str() {
                "header" => {
                    col = col.child(
                        div()
                            .flex_none()
                            .h(t.m("sideBarHeader.height", 35.0))
                            .px(t.edges("sideBarHeader.padding", 20.0).left)
                            .flex()
                            .items_center()
                            .text_size(t.m("sideBarTitle.fontSize", 11.0))
                            .font_weight(t.weight("sideBarTitle.weight", gpui::FontWeight::NORMAL))
                            .text_color(t.c("sideBarTitle.foreground"))
                            .child(title.clone()),
                    )
                }
                "content" => {
                    if let Some(c) = content.take() {
                        col = col.child(div().flex_1().min_h_0().child(c));
                    }
                }
                "activityBar" => col = col.child(self.render_activity_bar(false, true, cx)),
                _ => {}
            }
        }
        // The view itself can't be dropped, only moved.
        if let Some(c) = content {
            col = col.child(div().flex_1().min_h_0().child(c));
        }
        col.child(sash)
    }

    /// The editor column; `editor.order` arranges tabs, breadcrumbs, content and the panel.
    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let split = self.is_split();
        let tabs_at_bottom = lay.tabs_below_content();
        let show_tabs = lay.editor.iter().any(|p| p == "tabs");
        let mut content: Option<gpui::AnyElement> = Some(if split {
            // Two groups side by side, each with its own tab strip.
            let border = t.c_or("editorGroup.border", 0x80808040);
            let groups: Vec<_> = (0..2)
                .map(|g| {
                    let shown = self.visible[g].and_then(|i| self.tabs.get(i)).map(|t| (t.editor.clone(), t.page.clone()));
                    let editor: Option<gpui::AnyElement> = shown.map(|(editor, page)| self.render_page(page, editor, cx));
                    let tabs = show_tabs.then(|| self.render_tabs(tabs_at_bottom, g, cx));
                    let body = div().flex_1().min_h_0().children(editor);
                    div()
                        .id(SharedString::from(format!("editor-group-{g}")))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .when(g == 1, |d| d.border_l_1().border_color(border))
                        .map(|d| {
                            let tabs = tabs.map(|t| t.into_any_element());
                            if tabs_at_bottom { d.child(body).children(tabs) } else { d.children(tabs).child(body) }
                        })
                        .on_mouse_down(MouseButton::Left, cx.listener(move |wb, _, w, cx| {
                            if wb.focus_group != g {
                                wb.focus_group(g, w, cx);
                            }
                        }))
                        .into_any_element()
                })
                .collect();
            div().size_full().flex().children(groups).into_any_element()
        } else {
            match self.active.and_then(|i| self.tabs.get(i)).map(|t| (t.editor.clone(), t.page.clone())) {
                Some((editor, page)) => self.render_page(page, editor, cx),
                None => self.render_welcome(window, cx).into_any_element(),
            }
        });
        let mut col = div().flex_1().min_w_0().h_full().flex().flex_col();
        for part in &lay.editor {
            match part.as_str() {
                "tabs" if !self.tabs.is_empty() && !split => col = col.child(self.render_tabs(tabs_at_bottom, 0, cx)),
                "breadcrumbs" => col = col.children(self.render_breadcrumbs(cx)),
                "content" => {
                    if let Some(c) = content.take() {
                        col = col.child(div().flex_1().min_h_0().child(c));
                    }
                }
                "panel" if self.panel_visible && !lay.panel_in_main() => {
                    let place = if lay.panel_above_content() { PanelPlace::Top } else { PanelPlace::Bottom };
                    col = col.child(self.render_panel(place, cx));
                }
                _ => {}
            }
        }
        if let Some(c) = content {
            col = col.child(div().flex_1().min_h_0().child(c));
        }
        col
    }

    /// What a tab shows: its page, or its editor.
    fn render_page(&mut self, page: Option<Page>, editor: Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match page {
            Some(Page::Module(id)) => self.render_module_page(&id, cx).into_any_element(),
            Some(Page::Markdown(path)) => self.render_markdown_page(&path, cx).into_any_element(),
            None => editor.into_any_element(),
        }
    }

    fn render_tabs(&mut self, at_bottom: bool, group: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let indicator_bottom = t.layout.tab_indicator_bottom;
        let active = self.visible[group];
        let markdown_source = active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.page.is_none() && t.path.as_deref().is_some_and(preview::is_markdown));
        let tab_els: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.group == group)
            .map(|(i, tab)| {
                let is_active = active == Some(i);
                let modified = tab.editor.read(cx).is_modified();
                let fg = if is_active { t.c("tab.activeForeground") } else { t.c("tab.inactiveForeground") };
                let bg = if is_active { t.bg("tab.activeBackground") } else { t.bg("tab.inactiveBackground") };
                let hover = t.c("tab.hoverBackground");
                let close_hover = t.c("toolbar.hoverBackground");
                let indicator = t.m("tab.indicatorSize", 1.0);
                let group = SharedString::from(format!("tab-{i}"));
                let close = div()
                    .id(SharedString::from(format!("tab-close-{i}")))
                    .ml(t.edges("tab.closeMargin", 6.0).left)
                    .p(t.edges("tab.closePadding", 3.0).top)
                    .rounded(t.radius("tab.closeRadius", 4.0))
                    .hover(move |s| s.bg(close_hover))
                    .when(!is_active && !modified, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                    .map(|d| {
                        if modified {
                            d.child(
                                div()
                                    .size(t.m("tab.closeIconSize", 12.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().size(t.m("tab.modifiedDotSize", 8.0)).rounded_full().bg(t.c("tab.modifiedIndicator"))),
                            )
                        } else {
                            d.child(icon("close", t.m("tab.closeIconSize", 12.0), fg))
                        }
                    })
                    .on_click(cx.listener(move |wb, _, w, cx| {
                        cx.stop_propagation();
                        wb.close_tab(i, w, cx)
                    }));
                let margin = t.edges("tab.margin", 0.0);
                let padding = t.edges("tab.padding", 12.0);
                let border = t.edges("tab.borderThickness", 0.0);
                div()
                    .id(SharedString::from(format!("tab-{i}")))
                    .group(SharedString::from(format!("tab-{i}")))
                    .relative()
                    .h_full()
                    .min_w(t.m("tab.minWidth", 90.0))
                    .mt(margin.top)
                    .mb(margin.bottom)
                    .ml(margin.left)
                    .mr(margin.right)
                    .pl(padding.left)
                    .pr(padding.right)
                    .flex()
                    .items_center()
                    .bg(bg)
                    .rounded(t.radius("tab.radius", 0.0))
                    .border_color(if is_active { t.c("tab.activeBorder") } else { t.c("tab.border") })
                    .border_l(border.left)
                    .border_r(border.right)
                    .border_t(border.top)
                    .border_b(border.bottom)
                    .text_color(fg)
                    .text_size(t.m("tab.fontSize", 13.0))
                    .when(is_active, |d| d.font_weight(t.weight("tab.activeWeight", gpui::FontWeight::NORMAL)))
                    .when(t.italic("tab.style"), |d| d.italic())
                    .cursor_pointer()
                    .when(!is_active, |d| d.hover(move |s| s.bg(hover)))
                    .tooltip(tooltip(tab.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| tab.title.clone())))
                    .when(is_active && indicator > px(0.), |d| {
                        d.child(div().absolute().when(indicator_bottom, |d| d.bottom_0()).when(!indicator_bottom, |d| d.top_0()).left_0().right_0().h(indicator).rounded(t.radius("tab.indicatorRadius", 0.0)).bg(t.bg("tab.activeBorderTop")))
                    })
                    .child(div().whitespace_nowrap().child(tab.title.clone()))
                    .child(close)
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |wb, _, w, cx| wb.close_tab(i, w, cx)))
                    .on_click(cx.listener(move |wb, _, w, cx| wb.activate(i, w, cx)))
            })
            .collect();
        div()
            .id(SharedString::from(format!("tab-strip-{group}")))
            .flex_none()
            .h(t.m("tab.height", 35.0))
            .w_full()
            .flex()
            .gap(t.m("tab.spacing", 0.0))
            .px(t.edges("tabStrip.padding", 0.0).left)
            .overflow_x_scroll()
            .bg(t.bg("editorGroupHeader.background"))
            .map(|d| {
                let b = px(f32::from(t.edges("editorGroupHeader.borderThickness", 1.0).bottom));
                if at_bottom { d.border_t(b) } else { d.border_b(b) }
            })
            .border_color(t.c("editorGroupHeader.border"))
            .children(tab_els)
            .when(markdown_source, |d| {
                d.child(div().flex_1()).child(div().flex_none().h_full().flex().items_center().child(
                    div()
                        .id(SharedString::from(format!("md-preview-button-{group}")))
                        .mx_2()
                        .p(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                        .tooltip(tooltip("Open Preview to the Side (Ctrl+Shift+V opens it here)"))
                        .child(icon("openPreview", px(16.), t.c("icon.foreground")))
                        .on_click(cx.listener(|wb, _, w, cx| wb.open_markdown_preview(true, w, cx))),
                ))
            })
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = Theme::global(cx);
        let path = self.active.and_then(|i| self.tabs.get(i)).and_then(|tab| tab.path.clone())?;
        let display = match self.workspace.root() {
            Some(root) => path.strip_prefix(&root).map(Path::to_path_buf).unwrap_or(path.clone()),
            None => path.clone(),
        };
        let parts: Vec<String> = display.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let count = parts.len();
        let icon_size = t.m("breadcrumb.iconSize", 12.0);
        let fg = t.c("breadcrumb.foreground");
        Some(
            div()
                .flex_none()
                .h(t.m("breadcrumb.height", 22.0))
                .px(t.edges("breadcrumb.padding", 14.0).left)
                .flex()
                .items_center()
                .gap_1()
                .bg(t.bg("breadcrumb.background"))
                .border_b(px(f32::from(t.edges("breadcrumb.borderThickness", 0.0).bottom)))
                .border_color(t.c("breadcrumb.border"))
                .text_size(t.m("breadcrumb.fontSize", 12.0))
                .children(parts.into_iter().enumerate().flat_map(move |(i, part)| {
                    let color = if i + 1 == count { t.c("breadcrumb.activeForeground") } else { fg };
                    let sep = (i > 0).then(|| icon("chevronRight", icon_size, fg).into_any_element());
                    sep.into_iter().chain(std::iter::once(div().text_color(color).child(part).into_any_element()))
                })),
        )
    }

    /// The welcome page; `welcome.order` (logo, title, shortcuts) and `welcome.shortcuts` (command ids).
    fn render_welcome(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let commands = cx.global::<Commands>();
        let mut page = div().size_full().flex().flex_col().items_center().justify_center().gap(t.m("welcome.spacing", 8.0)).bg(t.bg("editor.background"));
        for part in &t.layout.welcome {
            match part.as_str() {
                "logo" => page = page.child(icon("logo", t.m("welcome.logoSize", 96.0), t.c("welcome.logo"))),
                "title" => {
                    page = page.child(
                        div()
                            .text_size(t.m("welcome.titleSize", 44.0))
                            .font_weight(t.weight("welcome.titleWeight", gpui::FontWeight::LIGHT))
                            .text_color(t.c("welcome.titleForeground"))
                            .child(t.layout.welcome_title.clone()),
                    )
                }
                "shortcuts" => {
                    for id in &t.layout.welcome_commands {
                        let Some(cmd) = commands.get(id) else { continue };
                        let key = pretty_keys(&window.keystroke_text_for(cmd.action.as_ref()));
                        let key = if key.contains("::") { String::new() } else { key };
                        let action = cmd.action.boxed_clone();
                        page = page.child(
                            div()
                                .id(SharedString::from(format!("welcome-{id}")))
                                .flex()
                                .gap_3()
                                .cursor_pointer()
                                .text_color(t.c("welcome.foreground"))
                                .hover(|s| s.text_color(t.c("welcome.titleForeground")))
                                .child(div().w(t.m("welcome.columnWidth", 160.0)).flex().justify_end().child(cmd.title.clone()))
                                .child(div().w(t.m("welcome.columnWidth", 160.0)).child(key))
                                .on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)),
                        );
                    }
                }
                _ => {}
            }
        }
        page
    }

    fn render_panel(&mut self, place: PanelPlace, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let problems = self.problems(cx);
        let tab = |id: SharedString, label: String, active: bool| {
            let border = t.edges("panelTab.indicator.border", 1.0);
            div()
                .id(id)
                .px(t.edges("panelTab.padding", 4.0).left)
                .py(t.edges("panelTab.padding", 6.0).top)
                .mx(t.edges("panelTab.margin", 6.0).left)
                .rounded(t.radius("panelTab.radius", 0.0))
                .text_size(t.m("panelTab.fontSize", 11.0))
                .cursor_pointer()
                .text_color(if active { t.c("panelTitle.activeForeground") } else { t.c("panelTitle.inactiveForeground") })
                .when(active, |d| d.bg(t.bg("panelTitle.activeBackground")).border_b(border.bottom).border_color(t.c("panelTitle.activeBorder")))
                .child(label)
        };
        let (height, width) = (self.panel_height, self.panel_width);
        let sash_hover = t.c("sash.hoverBorder");
        let content: gpui::AnyElement = match self.panel {
            PanelView::Output => self.output.clone().into_any_element(),
            PanelView::DebugConsole => self.render_debug_console(cx),
            PanelView::References => self.render_references(cx),
            PanelView::Terminal => match self.terminals.get(self.active_terminal) {
                Some((terminal, _)) => terminal.clone().into_any_element(),
                None => div().p_3().text_color(t.c("foreground.muted")).child("No terminal is open. Press Ctrl+Shift+` to start one.").into_any_element(),
            },
            PanelView::Problems => {
                let root = self.workspace.root();
                let icon_size = t.m("panel.iconSize", 14.0);
                let rows: Vec<_> = problems
                    .iter()
                    .take(2000)
                    .enumerate()
                    .map(|(i, (path, d))| {
                        let (name, color_key) = match d.severity {
                            bf_lsp::Severity::Error => ("error", "problemsErrorIcon.foreground"),
                            bf_lsp::Severity::Warning => ("warning", "problemsWarningIcon.foreground"),
                            _ => ("info", "problemsInfoIcon.foreground"),
                        };
                        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path).display().to_string();
                        let (p, at) = (path.clone(), d.start);
                        div()
                            .id(i)
                            .px_3()
                            .py(px(2.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("list.hoverBackground")))
                            .child(icon(name, icon_size, t.c(color_key)))
                            .child(div().whitespace_nowrap().child(d.message.clone()))
                            .child(div().text_color(t.c("foreground.muted")).whitespace_nowrap().child(format!(
                                "{}{rel} [Ln {}, Col {}]",
                                d.code.as_ref().map(|c| format!("{c}  ")).unwrap_or_default(),
                                d.start.line + 1,
                                d.start.col + 1
                            )))
                            .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(p.clone(), Some(at), w, cx)))
                    })
                    .collect();
                div()
                    .id("problems")
                    .size_full()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |d| d.child(div().p_3().text_color(t.c("foreground.muted")).child("No problems have been detected in the open files.")))
                    .children(rows)
                    .into_any_element()
            }
        };
        let errors = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Error).count();
        let thickness = px(f32::from(t.edges("panel.borderThickness", 1.0).top));
        let horizontal = matches!(place, PanelPlace::Top | PanelPlace::Bottom);
        let sash = div()
            .id("panel-sash")
            .absolute()
            .hover(move |s| s.bg(sash_hover))
            .map(|d| match place {
                PanelPlace::Bottom => d.top(px(-2.)).left_0().right_0().h(px(5.)),
                PanelPlace::Top => d.bottom(px(-2.)).left_0().right_0().h(px(5.)),
                PanelPlace::Left => d.right(px(-2.)).top_0().bottom_0().w(px(5.)),
                PanelPlace::Right => d.left(px(-2.)).top_0().bottom_0().w(px(5.)),
            })
            .cursor(if horizontal { gpui::CursorStyle::ResizeUpDown } else { gpui::CursorStyle::ResizeLeftRight })
            .on_mouse_down(MouseButton::Left, cx.listener(move |wb, e: &MouseDownEvent, _, cx| {
                wb.sash = Some(match place {
                    PanelPlace::Bottom | PanelPlace::Top => Sash::Panel { start_y: f32::from(e.position.y), start_h: height, top: place == PanelPlace::Top },
                    PanelPlace::Left | PanelPlace::Right => Sash::PanelSide { start_x: f32::from(e.position.x), start_w: width, right: place == PanelPlace::Right },
                });
                cx.stop_propagation();
            }));
        let mut header = div().flex_none().h(t.m("panelHeader.height", 35.0)).px(t.edges("panelHeader.padding", 8.0).left).flex().items_center();
        // The Debug Console is always available, even when a theme lists the tabs without it.
        // The Debug Console and the terminal are always available, even when a theme lists the tabs without them.
        let mut panel_tabs = t.layout.panel_tabs.clone();
        for required in ["debugConsole", "terminal"] {
            if !panel_tabs.iter().any(|n| n == required) {
                panel_tabs.push(required.into());
            }
        }
        // References shows up once there's a result.
        if self.references.is_some() && !panel_tabs.iter().any(|n| n == "references") {
            panel_tabs.push("references".into());
        }
        for name in &panel_tabs {
            match name.as_str() {
                "references" => {
                    let count = self.references.as_ref().map_or(0, |r| r.items.len());
                    header = header.child(
                        tab("ptab-references".into(), format!("REFERENCES ({count})"), self.panel == PanelView::References)
                            .on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::References, cx))),
                    );
                }
                "problems" => {
                    let label = format!("PROBLEMS{}", if errors > 0 { format!(" ({errors})") } else { String::new() });
                    header = header.child(tab("ptab-problems".into(), label, self.panel == PanelView::Problems).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))));
                }
                "output" => {
                    header = header.child(tab("ptab-output".into(), "OUTPUT".into(), self.panel == PanelView::Output).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Output, cx))));
                }
                "debugConsole" => {
                    header = header.child(
                        tab("ptab-debug".into(), "DEBUG CONSOLE".into(), self.panel == PanelView::DebugConsole)
                            .on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::DebugConsole, cx))),
                    );
                }
                "terminal" => {
                    header = header.child(
                        tab("ptab-terminal".into(), "TERMINAL".into(), self.panel == PanelView::Terminal)
                            .on_click(cx.listener(|wb, _, w, cx| wb.show_terminal(w, cx))),
                    );
                }
                _ => {}
            }
        }
        header = header.child(div().flex_1());
        if self.panel == PanelView::Terminal {
            header = header.children(self.render_terminal_tabs(cx));
        }
        header = header.child(
            div()
                .id("panel-close")
                .p_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .child(icon("close", t.m("panel.iconSize", 14.0), t.c("icon.foreground")))
                .on_click(cx.listener(|wb, _, _, cx| {
                    wb.panel_visible = false;
                    cx.notify();
                })),
        );
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.bg("panel.background"))
            .border_color(t.c("panel.border"))
            .map(|d| match place {
                PanelPlace::Bottom => d.w_full().h(px(height)).border_t(thickness),
                PanelPlace::Top => d.w_full().h(px(height)).border_b(thickness),
                PanelPlace::Left => d.h_full().w(px(width)).border_r(thickness),
                PanelPlace::Right => d.h_full().w(px(width)).border_l(thickness),
            })
            .child(sash)
            .child(header)
            .child(div().flex_1().min_h_0().child(content))
    }

    /// The status bar: `statusBar.left/center/right` list the items of each zone (branch, sync, problems,
    /// errors, warnings, run, modules, position, indent, encoding, eol, language, lsp, spacer, commands).
    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let fg = t.c("statusBar.foreground");
        let hover = t.c("statusBarItem.hoverBackground");
        let icon_size = t.m("statusBar.iconSize", 13.0);
        let pad = t.edges("statusBarItem.padding", 8.0).left;
        let radius = t.radius("statusBarItem.radius", 0.0);
        // Each part can be styled on its own: statusBarItem.<part>.background / .foreground (colors) and
        // statusBarItem.<part>.weight (fonts), e.g. "statusBarItem.text:NORMAL.background".
        let item = |part: &str, id: SharedString, icon_name: Option<&str>, text: String| {
            let (bg_key, fg_key) = (format!("statusBarItem.{part}.background"), format!("statusBarItem.{part}.foreground"));
            let color = if t.has(&fg_key) { t.c(&fg_key) } else { fg };
            div()
                .id(id)
                .flex_none()
                .h_full()
                .px(pad)
                .flex()
                .items_center()
                .gap_1()
                .rounded(radius)
                .cursor_pointer()
                .when(t.has(&bg_key), |d| d.bg(t.bg(&bg_key)))
                .text_color(color)
                .font_weight(t.weight(&format!("statusBarItem.{part}.weight"), gpui::FontWeight::NORMAL))
                .hover(move |s| s.bg(hover))
                .children(icon_name.map(|n| icon(n, icon_size, color)))
                .when(!text.is_empty(), |d| d.child(text))
        };
        let problems = self.problems(cx);
        let errors = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Error).count();
        let warnings = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Warning).count();
        let running = self.dotnet.as_ref().is_some_and(DotnetProcess::is_running) || self.debug.is_some();
        let debugging = self.debug.as_ref().map(|d| d.status_text());
        let git = self.scm.read(cx).git().is_some().then(|| self.scm.read(cx).status().clone());
        let bom = self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.bom);
        // (position, indent, encoding, eol, language) of the active editor.
        let on_page = self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.page.is_some());
        let editor_info = self.active_editor().filter(|_| !on_page).map(|editor| {
            let e = editor.read(cx);
            let caret = e.state.caret();
            let selection = if e.state.has_selection() { format!(" ({} selected)", e.state.selected_text().chars().count()) } else { String::new() };
            let indent = if e.state.insert_spaces { format!("Spaces: {}", e.state.tab_size) } else { format!("Tab Size: {}", e.state.tab_size) };
            let eol = match e.state.buffer.line_ending() {
                bf_text::LineEnding::Lf => "LF",
                bf_text::LineEnding::CrLf => "CRLF",
            };
            let position = t.layout.position_format.replace("{line}", &(caret.line + 1).to_string()).replace("{col}", &(caret.col + 1).to_string()).replace("{lines}", &e.state.buffer.line_count().to_string());
            (format!("{position}{selection}"), indent, if bom { "UTF-8 with BOM" } else { "UTF-8" }, eol, e.language.map_or("Plain Text", |l| l.name()))
        });

        // Module items go to the zone matching their alignment if it lists "modules", else the first zone that does.
        let zones: [&Vec<String>; 3] = [&lay.status_left, &lay.status_center, &lay.status_right];
        let has_modules = |z: usize| zones[z].iter().any(|n| n == "modules");
        let module_zone = |right: bool| {
            let wanted = if right { 2 } else { 0 };
            if has_modules(wanted) { Some(wanted) } else { (0..3).find(|z| has_modules(*z)) }
        };

        let mut out: [Vec<gpui::AnyElement>; 3] = Default::default();
        for (z, names) in zones.iter().enumerate() {
            for (n, name) in names.iter().enumerate() {
                let id = |suffix: &str| SharedString::from(format!("st-{z}-{n}-{suffix}"));
                let el: Vec<gpui::AnyElement> = match name.as_str() {
                    "branch" => git.as_ref().map(|s| {
                        let branch = s.branch.clone().unwrap_or_else(|| if s.detached { "(detached)".into() } else { "—".into() });
                        let dirty = if s.changes.is_empty() { "" } else { "*" };
                        item(name, id("b"), Some("sourceControl"), format!("{branch}{dirty}")).tooltip(tooltip("Checkout branch…")).on_click(cx.listener(|wb, _, w, cx| wb.open_branch_picker(w, cx))).into_any_element()
                    }).into_iter().collect(),
                    "sync" => git.as_ref().filter(|s| s.has_commits).map(|s| {
                        let (text, tip) = match (&s.upstream, s.ahead, s.behind) {
                            (Some(_), 0, 0) => (String::new(), "Synchronize changes (pull, then push)"),
                            (Some(_), a, b) => (format!("{b}↓ {a}↑"), "Synchronize changes (pull, then push)"),
                            (None, _, _) => (String::new(), "Publish branch to origin"),
                        };
                        item(name, id("s"), Some(if s.upstream.is_some() { "sync" } else { "arrowUp" }), text).tooltip(tooltip(tip)).on_click(cx.listener(|wb, _, _, cx| wb.scm.update(cx, |s, cx| s.sync(cx)))).into_any_element()
                    }).into_iter().collect(),
                    "problems" => vec![
                        item(name, id("e"), Some("error"), errors.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element(),
                        item(name, id("w"), Some("warning"), warnings.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element(),
                    ],
                    "errors" => vec![item(name, id("e"), Some("error"), errors.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element()],
                    "warnings" => vec![item(name, id("w"), Some("warning"), warnings.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element()],
                    "run" => vec![
                        item(name, id("r"), Some(if running { "stop" } else { "play" }), debugging.clone().unwrap_or_else(|| if running { "Stop".into() } else { "Run".into() }))
                            .tooltip(tooltip(if running { "Stop (Shift+F5)" } else { "Build and run without debugging (Ctrl+F5)" }))
                            .on_click(cx.listener(move |_, _, w, cx| {
                                if running {
                                    w.dispatch_action(Box::new(Stop), cx)
                                } else {
                                    w.dispatch_action(Box::new(Run), cx)
                                }
                            }))
                            .into_any_element(),
                    ],
                    "modules" => self
                        .plugin_status
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| module_zone(e.right) == Some(z) && !e.text.is_empty())
                        .map(|(i, entry)| {
                            let command = entry.command.clone();
                            let mut el = item(name, id(&format!("m{i}")), None, entry.text.clone()).on_click(cx.listener(move |_, _, w, cx| {
                                if let Some(id) = &command {
                                    w.dispatch_action(Box::new(RunModuleCommand { id: id.clone() }), cx);
                                }
                            }));
                            if let Some(tip) = entry.tooltip.clone() {
                                el = el.tooltip(tooltip(tip));
                            }
                            el.into_any_element()
                        })
                        .collect(),
                    "position" | "indent" | "encoding" | "eol" | "language" => {
                        let Some(info) = &editor_info else { continue };
                        let text = match name.as_str() {
                            "position" => info.0.clone(),
                            "indent" => info.1.clone(),
                            "encoding" => info.2.into(),
                            "eol" => info.3.into(),
                            _ => info.4.into(),
                        };
                        let el = item(name, id(name), None, text);
                        vec![match name.as_str() {
                            "position" => el.on_click(cx.listener(|wb, _, w, cx| wb.open_palette(Mode::GoToLine, w, cx))).into_any_element(),
                            "indent" => el.on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(OpenSettings), cx))).into_any_element(),
                            "eol" => el.on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(ToggleLineEnding), cx))).into_any_element(),
                            _ => el.into_any_element(),
                        }]
                    }
                    "lsp" => (!self.lsp_status.is_empty())
                        .then(|| item(name, id("c"), None, self.lsp_status.clone()).tooltip(tooltip("C# language server (csharp-ls) — click to restart")).on_click(cx.listener(|wb, _, _, cx| wb.start_lsp(cx))).into_any_element())
                        .into_iter()
                        .collect(),
                    "spacer" => vec![div().flex_1().into_any_element()],
                    "file" => self
                        .active
                        .and_then(|i| self.tabs.get(i))
                        .map(|tab| {
                            let modified = if tab.editor.read(cx).is_modified() { " [+]" } else { "" };
                            item(name, id("f"), None, format!("{}{modified}", tab.title)).into_any_element()
                        })
                        .into_iter()
                        .collect(),
                    other if other.starts_with("text:") => vec![item(name, id("t"), None, other["text:".len()..].to_owned()).into_any_element()],
                    other => command_button(other, cx)
                        .map(|(action, title, icon_name)| {
                            let explicit_icon = crate::layout::command_item(other).and_then(|(_, i)| i).is_some();
                            let el = if explicit_icon { item(name, id("x"), Some(&icon_name), String::new()) } else { item(name, id("x"), None, title.clone()) };
                            el.tooltip(tooltip(title)).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).into_any_element()
                        })
                        .into_iter()
                        .collect(),
                };
                out[z].extend(el);
            }
        }
        let [left, center, right] = out;
        div()
            .flex_none()
            .h(t.m("statusBar.height", 24.0))
            .w_full()
            .flex()
            .bg(if self.debug.is_some() && t.has("statusBar.debuggingBackground") { t.bg("statusBar.debuggingBackground") } else { t.bg("statusBar.background") })
            .text_color(fg)
            .text_size(t.m("statusBar.fontSize", 12.0))
            .map(|d| {
                let b = px(f32::from(t.edges("statusBar.borderThickness", 1.0).top));
                if lay.workbench.iter().position(|p| p == "statusBar") < lay.workbench.iter().position(|p| p == "main") { d.border_b(b) } else { d.border_t(b) }
            })
            .border_color(t.c("statusBar.border"))
            .child(div().flex_1().min_w_0().h_full().flex().items_center().overflow_hidden().children(left))
            .child(div().flex_none().h_full().flex().items_center().children(center))
            .child(div().flex_1().min_w_0().h_full().flex().items_center().justify_end().overflow_hidden().children(right))
    }
}

/// A command placed in a bar: `command:<id>`, `command:<id>:<icon>` or a bare command id.
/// Returns the action, its title and an icon name.
fn command_button(item: &str, cx: &App) -> Option<(Box<dyn gpui::Action>, String, String)> {
    let (id, icon_name) = crate::layout::command_item(item).unwrap_or((item, None));
    let cmd = cx.global::<Commands>().get(id)?;
    let icon_name = icon_name.map(str::to_owned).unwrap_or_else(|| default_command_icon(id).to_owned());
    Some((cmd.action.boxed_clone(), cmd.display_title(), icon_name))
}

fn default_command_icon(id: &str) -> &'static str {
    match id {
        "dotnet.build" => "build",
        "dotnet.run" | "debug.start" => "play",
        "debug.restart" => "refresh",
        "debug.pause" => "pause",
        "debug.stepOver" => "stepOver",
        "debug.stepInto" => "stepInto",
        "debug.stepOut" => "stepOut",
        "debug.toggleBreakpoint" | "debug.removeAllBreakpoints" | "debug.attach" | "view.debug" => "debug",
        "view.debugConsole" => "layoutPanel",
        "terminal.toggle" | "terminal.new" => "terminal",
        "terminal.kill" => "trash",
        "terminal.selectDefaultShell" => "terminal",
        "dotnet.stop" => "stop",
        "theme.select" | "theme.customize" | "theme.edit" => "palette",
        "view.commandPalette" | "view.quickOpen" | "view.search" => "search",
        "view.explorer" => "files",
        "view.scm" | "git.checkout" | "git.createBranch" | "git.init" => "sourceControl",
        "git.pull" => "arrowDown",
        "git.push" => "arrowUp",
        "git.sync" | "git.fetch" => "sync",
        "git.refresh" | "theme.reload" | "csharp.reload" => "refresh",
        "git.commit" => "check",
        "git.openChanges" => "diff",
        "git.stageAll" => "add",
        "git.unstageAll" => "remove",
        "settings.open" => "settings",
        "view.toggleSideBar" => "layoutSideBar",
        "view.togglePanel" | "view.problems" | "view.output" => "layoutPanel",
        "view.modules" => "puzzle",
        "test.runAll" | "test.runAtCursor" => "play",
        "view.testing" | "test.refresh" => "beaker",
        "test.debugAtCursor" => "debug",
        "test.rerunFailed" => "refresh",
        "file.newFile" | "file.openFile" | "file.save" | "file.saveAs" | "file.saveAll" => "file",
        "file.openFolder" => "folderOpen",
        "file.close" | "file.closeAll" | "app.exit" => "close",
        "app.about" => "info",
        _ => "puzzle",
    }
}

fn render_window_controls(window: &Window, cx: &App) -> impl IntoElement + use<> {
    let t = Theme::global(cx);
    let maximized = window.is_maximized();
    let win_w = t.m("windowControl.width", 46.0);
    let wc_icon = t.m("windowControl.iconSize", 14.0);
    let wc_fg = t.c("windowControl.foreground");
    let wc_hover = t.c("windowControl.hoverBackground");
    let close_hover = t.c("windowControl.closeHoverBackground");
    let close_fg = t.c("windowControl.closeHoverForeground");
    div()
        .flex_none()
        .flex()
        .h_full()
        .child(
            div().id("wc-min").occlude().w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(wc_hover))
                .window_control_area(WindowControlArea::Min)
                .child(icon("windowMinimize", wc_icon, wc_fg))
                .on_click(|_, window, _| window.minimize_window()),
        )
        .child(
            div().id("wc-max").occlude().w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(wc_hover))
                .window_control_area(WindowControlArea::Max)
                .child(icon(if maximized { "windowRestore" } else { "windowMaximize" }, wc_icon, wc_fg))
                .on_click(|_, window, _| window.zoom_window()),
        )
        .child(
            div().id("wc-close").occlude().group("wc-close").w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(close_hover))
                .window_control_area(WindowControlArea::Close)
                .child(icon("windowClose", wc_icon, wc_fg).group_hover("wc-close", move |s| s.text_color(close_fg)))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Quit), cx)),
        )
}


fn side_view_named(name: &str) -> Option<SideView> {
    match name.to_ascii_lowercase().as_str() {
        "explorer" => Some(SideView::Explorer),
        "search" => Some(SideView::Search),
        "scm" | "sourcecontrol" | "git" => Some(SideView::SourceControl),
        "debug" | "run" => Some(SideView::Debug),
        "testing" | "tests" | "test" => Some(SideView::Testing),
        "solution" | "projects" => Some(SideView::Solution),
        "modules" => Some(SideView::Modules),
        _ => None,
    }
}

fn default_key(id: &str) -> &'static str {
    match id {
        "file.newFile" => "Ctrl+N",
        "file.openFile" => "Ctrl+O",
        "file.openFolder" => "Ctrl+Shift+O",
        "file.save" => "Ctrl+S",
        "file.saveAs" => "Ctrl+Shift+S",
        "file.saveAll" => "Ctrl+Alt+S",
        "file.close" => "Ctrl+W",
        "file.closeAll" => "Ctrl+Shift+W",
        "app.exit" => "Ctrl+Q",
        "edit.undo" => "Ctrl+Z",
        "edit.redo" => "Ctrl+Y",
        "edit.find" => "Ctrl+F",
        "edit.replace" => "Ctrl+H",
        "edit.toggleComment" => "Ctrl+/",
        "edit.duplicateLines" => "Shift+Alt+Down",
        "edit.addCursorAbove" => "Ctrl+Alt+Up",
        "edit.addCursorBelow" => "Ctrl+Alt+Down",
        "edit.addNextMatch" => "Ctrl+D",
        "edit.selectAllMatches" => "Ctrl+Shift+L",
        "edit.triggerSuggest" => "Ctrl+Space",
        "view.commandPalette" => "Ctrl+Shift+P",
        "view.quickOpen" => "Ctrl+P",
        "edit.goToLine" => "Ctrl+G",
        "edit.goToDefinition" => "F12",
        "edit.findReferences" => "Shift+F12",
        "edit.goToImplementation" => "Ctrl+F12",
        "edit.fold" => "Ctrl+Shift+[",
        "edit.unfold" => "Ctrl+Shift+]",
        "view.goToWorkspaceSymbol" => "Ctrl+T",
        "edit.rename" => "F2",
        "edit.signatureHelp" => "Ctrl+Shift+Space",
        "view.explorer" => "Ctrl+Shift+E",
        "view.scm" => "Ctrl+Shift+G",
        "view.search" => "Ctrl+Shift+F",
        "view.problems" => "Ctrl+Shift+M",
        "view.output" => "Ctrl+Shift+U",
        "view.toggleSideBar" => "Ctrl+B",
        "view.togglePanel" => "Ctrl+J",
        "view.nextEditor" => "Ctrl+Tab",
        "view.previousEditor" => "Ctrl+Shift+Tab",
        "dotnet.build" => "Ctrl+Shift+B",
        "dotnet.run" => "Ctrl+F5",
        "debug.start" => "F5",
        "debug.restart" => "Ctrl+Shift+F5",
        "debug.pause" => "F6",
        "debug.stepOver" => "F10",
        "debug.stepInto" => "F11",
        "debug.stepOut" => "Shift+F11",
        "debug.toggleBreakpoint" => "F9",
        "view.debug" => "Ctrl+Shift+D",
        "view.debugConsole" => "Ctrl+Shift+Y",
        "terminal.toggle" => "Ctrl+`",
        "terminal.new" => "Ctrl+Shift+`",
        "dotnet.stop" => "Shift+F5",
        "settings.open" => "Ctrl+,",
        "theme.select" => "Ctrl+K",
        _ => "",
    }
}