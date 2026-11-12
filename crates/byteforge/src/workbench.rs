//! The workbench: window layout, tabs, panels, title bar, menus, status bar and all workbench commands.

mod bars;
mod debug;
mod dotnet;
mod editor_area;
mod ef;
mod files;
mod git;
mod lsp;
mod module_views;
mod modules;
mod nuget;
mod palette;
mod panel;
mod plugins;
mod preview;
mod references;
mod render;
mod settings_page;
mod solution;
mod symbols;
mod tabs;
mod templates;
mod terminals;
mod testing;
mod title_bar;
mod update;

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
use crate::editor::{AfterApply, EditorEvent, EditorView};
use tabs::AutoSave;
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
    /// A diff editor (the tab's editor is its new side).
    Diff(Entity<crate::diff_view::DiffView>),
    /// The settings screen.
    Settings,
    /// The user secrets of a project, as a form (`file` is its secrets.json).
    Secrets { project: PathBuf, file: PathBuf },
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
    /// Name for a new project or item from this template.
    Template(crate::actions::UseTemplate),
    RenameSymbol { editor: gpui::WeakEntity<EditorView>, at: Point, old: String },
    /// Name of a new EF Core migration.
    EfMigrationName(crate::actions::EfCommand),
    /// The connection string of the database to scaffold a DbContext from, into this project.
    EfScaffoldConnection { project: PathBuf },
    /// New value of a user secret.
    SecretValue { file: PathBuf, key: String },
    /// A new user secret, typed as `Name = value`.
    SecretNew { file: PathBuf },
    /// A new watch expression.
    WatchExpression,
    /// A value for a setting on the settings screen.
    SettingValue { key: String },
    /// A breakpoint's condition, hit count or log message.
    BreakpointOption { path: PathBuf, line: usize, option: debug::BreakpointOption },
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

pub(crate) const MENUS: &[(&str, &[&str])] = &[
    ("File", &["file.newFile", "dotnet.newItem", "dotnet.newProject", "-", "file.openFile", "file.openFolder", "-", "file.save", "file.saveAs", "file.saveAll", "-", "file.close", "file.closeAll", "-", "app.exit"]),
    ("Edit", &["edit.undo", "edit.redo", "-", "edit.find", "edit.replace", "-", "edit.toggleComment", "edit.duplicateLines", "edit.triggerSuggest", "edit.signatureHelp", "-", "edit.formatDocument", "edit.toggleLineEnding"]),
    ("Refactor", &[
        "refactor.menu", "edit.quickFix", "-", "edit.rename", "-", "refactor.extractMethod", "refactor.extractLocalFunction", "refactor.extractInterface",
        "refactor.extractBaseClass", "-", "refactor.introduceVariable", "refactor.introduceConstant", "refactor.introduceField", "refactor.introduceParameter",
        "-", "refactor.inline", "refactor.moveTypeToFile", "-", "refactor.removeAndSortUsings", "refactor.sortUsings",
    ]),
    ("View", &["view.commandPalette", "view.quickOpen", "-", "view.explorer", "view.search", "view.scm", "view.debug", "view.testing", "view.modules", "view.problems", "view.output", "view.debugConsole", "-", "terminal.toggle", "terminal.new", "terminal.selectDefaultShell", "-", "view.splitEditor", "view.openInOtherGroup", "-", "view.toggleWordWrap", "view.toggleSideBar", "view.togglePanel"]),
    ("Go", &["view.quickOpen", "edit.goToLine", "edit.goToDefinition", "edit.goToImplementation", "edit.goToTypeDefinition", "edit.findReferences", "-", "view.goToSymbol", "view.goToWorkspaceSymbol", "-", "view.nextEditor", "view.previousEditor"]),
    ("Git", &["git.commit", "git.refresh", "-", "git.pull", "git.push", "git.sync", "git.fetch", "-", "git.checkout", "git.createBranch", "-", "git.stageAll", "git.unstageAll", "git.openChanges", "-", "git.fileHistory", "git.showLineCommit", "git.toggleBlame", "git.toggleBlameGutter", "-",
        "merge.acceptCurrent", "merge.acceptIncoming", "merge.acceptBoth", "merge.next", "merge.previous", "-", "git.init"]),
    ("Run", &[
        "debug.start", "dotnet.run", "dotnet.watch", "dotnet.restartWatch", "debug.restart", "dotnet.stop", "-", "dotnet.selectLaunchProfile", "dotnet.userSecrets", "-", "debug.stepOver", "debug.stepInto", "debug.stepOut", "debug.pause", "-",
        "debug.toggleBreakpoint", "debug.conditionalBreakpoint", "debug.logpoint", "debug.hitCount", "debug.removeAllBreakpoints", "debug.attach", "debug.attachContainer", "-", "dotnet.build", "dotnet.test", "dotnet.clean", "dotnet.restore", "-",
        "ef.addMigration", "ef.removeMigration", "ef.updateDatabase", "ef.updateDatabaseTo", "ef.script", "ef.scaffold", "ef.dropDatabase", "-",
        "test.runAll", "test.rerunFailed", "test.runAtCursor", "test.debugAtCursor", "-",
        "csharp.reload",
    ]),
    ("Preferences", &["settings.open", "settings.openJson", "-", "theme.select", "theme.customize", "theme.edit", "theme.reload"]),
    ("Help", &["view.modules", "-", "app.checkForUpdates", "app.about"]),
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
    /// The project the NuGet search in the palette adds to.
    package_target: Option<PathBuf>,
    package_search: Option<Task<()>>,
    /// `dotnet new list` results, loaded the first time New Project / New Item opens.
    project_templates: Option<Vec<bf_core::templates::Template>>,
    item_templates: Option<Vec<bf_core::templates::Template>>,
    template_task: Option<Task<()>>,
    /// The startup project's launch profile, shown in the status bar ("launchProfile").
    status_profile: Option<String>,
    update: update::UpdateState,
    update_task: Option<Task<()>>,
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
    /// The settings screen's search box.
    settings_search: Option<Entity<TextInput>>,
    /// User-secrets pages showing their values (hidden by default).
    secrets_shown: std::collections::HashSet<PathBuf>,
    /// The project the running (or last) `dotnet watch` was started for, so it can be restarted.
    watch_target: Option<PathBuf>,
    /// Compiler errors and warnings from the last build (Razor files only get diagnostics this way).
    build_problems: Vec<(PathBuf, crate::editor::Diagnostic)>,
    debug: Option<debug::DebugSession>,
    debug_generation: u64,
    /// Breakpoints of all files (open or not), sorted by line.
    breakpoints: BTreeMap<PathBuf, Vec<crate::editor::Breakpoint>>,
    /// Pending "afterDelay" auto saves, by editor.
    autosave: HashMap<gpui::EntityId, Task<()>>,
    /// Watch expressions of the opened folder (Debug view).
    watches: Vec<String>,
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
    activity_menu: Option<bars::ActivityMenu>,
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
                SearchEvent::Replace { files, line, matcher, template, count } => wb.replace_in_files(files.clone(), *line, matcher.clone(), template.clone(), *count, window, cx),
            }),
            cx.subscribe_in(&scm, window, Self::on_scm_event),
            // files.autoSave "onWindowChange": leaving the window saves everything.
            cx.observe_window_activation(window, |wb: &mut Self, window, cx| {
                if !window.is_window_active() && wb.autosave_mode() == AutoSave::OnWindowChange {
                    wb.save_all(window, cx);
                }
            }),
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
                            wb.refresh_status_profile();
        wb.schedule_update_check(window, cx);
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
            package_target: None,
            package_search: None,
            project_templates: None,
            item_templates: None,
            template_task: None,
            status_profile: None,
            update: update::UpdateState::Idle,
            update_task: None,
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
            watch_target: None,
            secrets_shown: Default::default(),
            settings_search: None,
            build_problems: Vec::new(),
            debug: None,
            debug_generation: 0,
            breakpoints: BTreeMap::new(),
            watches: Vec::new(),
            autosave: HashMap::new(),
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
            activity_menu: None,
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
        crate::explorer::set_native_icons(wb.settings.get("explorer.nativeIcons", true));
        wb.refresh_status_profile();
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
        crate::explorer::set_native_icons(self.settings.get("explorer.nativeIcons", true));
        self.refresh_status_profile();
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
        e.word_wrap = s.get_string("editor.wordWrap").is_some_and(|w| w == "on");
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
        self.load_watches();
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
                    wb.solution.update(cx, |s, cx| s.files_changed(cx));
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
        self.apply_pending_update();
    }

    // ───────────── C# language server ─────────────

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
        "dotnet.watch" => "flame",
        "dotnet.selectLaunchProfile" => "settings",
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

pub(crate) fn default_key(id: &str) -> String {
    let keys = match id {
        "file.newFile" => "ctrl-n",
        "file.openFile" => "ctrl-o",
        "file.openFolder" => "ctrl-shift-o",
        "file.save" => "ctrl-s",
        "file.saveAs" => "ctrl-shift-s",
        "file.saveAll" => "ctrl-alt-s",
        "file.close" => "ctrl-w",
        "file.closeAll" => "ctrl-shift-w",
        "app.exit" => "ctrl-q",
        "edit.undo" => "ctrl-z",
        "edit.redo" => "ctrl-y",
        "edit.find" => "ctrl-f",
        "edit.replace" => "ctrl-h",
        "edit.toggleComment" => "ctrl-/",
        "edit.duplicateLines" => "shift-alt-down",
        "edit.addCursorAbove" => "ctrl-alt-up",
        "edit.addCursorBelow" => "ctrl-alt-down",
        "edit.addNextMatch" => "ctrl-d",
        "edit.selectAllMatches" => "ctrl-shift-l",
        "edit.triggerSuggest" => "ctrl-space",
        "view.commandPalette" => "ctrl-shift-p",
        "view.quickOpen" => "ctrl-p",
        "edit.goToLine" => "ctrl-g",
        "edit.goToDefinition" => "f12",
        "edit.findReferences" => "shift-f12",
        "edit.goToImplementation" => "ctrl-f12",
        "edit.fold" => "ctrl-shift-[",
        "edit.unfold" => "ctrl-shift-]",
        "view.goToWorkspaceSymbol" => "ctrl-t",
        "view.goToSymbol" => "alt-\\",
        "edit.rename" => "f2",
        "view.toggleWordWrap" => "alt-z",
        "edit.quickFix" => "ctrl-.",
        "edit.formatDocument" => "shift-alt-f",
        "refactor.menu" => "ctrl-shift-r",
        "refactor.extractMethod" => "ctrl-r ctrl-m",
        "refactor.extractInterface" => "ctrl-r ctrl-i",
        "refactor.removeAndSortUsings" => "ctrl-r ctrl-g",
        "edit.signatureHelp" => "ctrl-shift-space",
        "view.explorer" => "ctrl-shift-e",
        "view.scm" => "ctrl-shift-g",
        "view.search" => "ctrl-shift-f",
        "view.problems" => "ctrl-shift-m",
        "view.output" => "ctrl-shift-u",
        "view.toggleSideBar" => "ctrl-b",
        "view.togglePanel" => "ctrl-j",
        "view.nextEditor" => "ctrl-tab",
        "view.previousEditor" => "ctrl-shift-tab",
        "dotnet.build" => "ctrl-shift-b",
        "dotnet.run" => "ctrl-f5",
        "dotnet.watch" => "alt-f5",
        "dotnet.restartWatch" => "shift-alt-f5",
        "debug.start" => "f5",
        "debug.restart" => "ctrl-shift-f5",
        "debug.pause" => "f6",
        "debug.stepOver" => "f10",
        "debug.stepInto" => "f11",
        "debug.stepOut" => "shift-f11",
        "debug.toggleBreakpoint" => "f9",
        "view.debug" => "ctrl-shift-d",
        "view.debugConsole" => "ctrl-shift-y",
        "terminal.toggle" => "ctrl-`",
        "terminal.new" => "ctrl-shift-`",
        "dotnet.stop" => "shift-f5",
        "settings.open" => "ctrl-,",
        "theme.select" => "ctrl-k",
        _ => "",
    };
    crate::actions::key_label(keys)
}