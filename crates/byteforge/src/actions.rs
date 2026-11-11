//! Actions, the command registry (what the command palette and menus list) and key bindings.

use std::collections::HashMap;

use gpui::{Action, App, KeyBinding, actions};

actions!(
    workbench,
    [
        NewFile, OpenFile, OpenFolder, Save, SaveAs, SaveAll, CloseEditor, CloseAllEditors, Quit, CommandPalette, QuickOpen, GoToLine,
        ToggleSideBar, ResetActivityBarOrder, TogglePanel, ShowExplorer, ShowSearch, ShowProblems, ShowOutput, ShowModules, NextEditor, PreviousEditor,
        SelectTheme, CustomizeTheme, EditTheme, ReloadTheme, OpenSettings, About, CheckForUpdates, Build, Run, RunWithWatch, SelectLaunchProfile, NewProject, NewItem, ManageUserSecrets, Test, Clean, Restore, Stop,
        ToggleLineEnding, ReloadCSharp, ShowSourceControl, GitRefresh, GitCommit, GitStageAll, GitUnstageAll, GitPull, GitPush,
        GitFetch, GitSync, GitCheckout, GitCreateBranch, GitInit, GitOpenChanges, ShowDebug, ShowDebugConsole, FormatDocument,
        FormatDocumentWith, GoToSymbolInEditor, GoToSymbolInWorkspace, SplitEditor, FocusFirstGroup, FocusSecondGroup,
        MarkdownOpenPreview, MarkdownOpenPreviewToSide, OpenInOtherGroup
    ]
);

/// The choice in a module's pick list (`quickPick`), sent back to the module.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = module, no_json)]
pub struct ModulePickChoice {
    pub module: String,
    pub request: u64,
    pub value: String,
}

/// Opens the NuGet search for this project (picked from the project list of "Add NuGet Package").
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = nuget, no_json)]
pub struct AddPackageTo {
    pub project: String,
}

actions!(nuget, [AddPackage, CheckPackageUpdates]);

/// Adds `id` at `version` to `project` (picked in the version list after a NuGet search).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = nuget, no_json)]
pub struct AddPackageVersion {
    pub project: String,
    pub id: String,
    pub version: String,
}

actions!(ef, [EfAddMigration, EfRemoveMigration, EfUpdateDatabase, EfUpdateDatabaseTo, EfGenerateScript, EfDropDatabase]);

/// An EF Core migrations command, filled in step by step (context picked, name typed, …): `op` is add,
/// remove, update, update-to, script or drop; empty fields are still to be asked.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = ef, no_json)]
pub struct EfCommand {
    pub op: String,
    pub project: String,
    pub context: String,
    pub arg: String,
}

actions!(explorer, [RenameEntry]);

actions!(terminal, [ToggleTerminal, NewTerminal, KillTerminal, TerminalCopy, TerminalPaste, SelectDefaultShell]);

actions!(
    debug,
    [StartDebugging, RestartDebugging, Pause, StepOver, StepInto, StepOut, RemoveAllBreakpoints, AttachToProcess, AttachToContainer]
);

actions!(
    testing,
    [ShowTesting, ShowSolution, RunAllTests, RerunFailedTests, RunTestAtCursor, DebugTestAtCursor, RefreshTests]
);

actions!(
    editor,
    [
        MoveLeft, MoveRight, MoveUp, MoveDown, SelectLeft, SelectRight, SelectUp, SelectDown, MoveWordLeft, MoveWordRight,
        SelectWordLeft, SelectWordRight, MoveHome, MoveEnd, SelectHome, SelectEnd, MoveDocStart, MoveDocEnd, SelectDocStart,
        SelectDocEnd, PageUp, PageDown, SelectPageUp, SelectPageDown, ScrollLineUp, ScrollLineDown, Backspace, DeleteWordLeft,
        Delete, DeleteWordRight, Newline, Tab, Outdent, SelectAll, Copy, Cut, Paste, Undo, Redo, DuplicateLines, DeleteLines,
        MoveLinesUp, MoveLinesDown, ToggleComment, Find, Replace, FindNext, FindPrevious, TriggerCompletion, GoToDefinition,
        Cancel, ToggleBreakpoint, QuickFix, FindReferences, RenameSymbol, ShowSignatureHelp, GoToImplementation, GoToTypeDefinition, Fold, Unfold, FoldAll, UnfoldAll,
        AddCursorAbove, AddCursorBelow, AddNextMatch, SelectAllMatches
    ]
);

actions!(diff, [DiffNextChange, DiffPreviousChange, DiffToggleInline]);

// Git in the editor: merge conflicts and blame (handled by the editor), file history (the workbench).
actions!(merge, [AcceptCurrentChange, AcceptIncomingChange, AcceptBothChanges, NextConflict, PreviousConflict]);
actions!(git, [ShowLineCommit, GitFileHistory, GitToggleBlame]);

/// Opens what `commit` changed in `file` (picked in File History).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = git, no_json)]
pub struct GitShowFileAtCommit {
    pub commit: bf_core::git::Commit,
    pub file: bf_core::git::CommitFile,
}

// Refactorings (handled by the editor; most go through the language server's code actions).
actions!(
    refactor,
    [
        Refactor, ExtractMethod, ExtractLocalFunction, IntroduceVariable, IntroduceConstant, IntroduceField, IntroduceParameter, InlineSymbol,
        ExtractInterface, ExtractBaseClass, MoveTypeToFile, RemoveAndSortUsings, SortUsings
    ]
);

actions!(input, [Confirm, InputBackspace, InputDelete, InputLeft, InputRight, InputSelectLeft, InputSelectRight, InputHome, InputEnd, InputSelectAll, InputCopy, InputCut, InputPaste, InputUp, InputDown, InputCancel, InputPageUp, InputPageDown]);

// The macOS application and Window menus.
actions!(app, [HideApp, HideOtherApps, ShowAllApps, MinimizeWindow, ZoomWindow]);

/// Attaches the debugger to a running .NET process (picked in the attach palette).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = debug, no_json)]
pub struct AttachToPid {
    pub pid: u32,
}

/// Lists the processes of this container to attach to (picked in "Attach to .NET Process in Container").
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = debug, no_json)]
pub struct PickContainerProcess {
    pub container: String,
}

/// Attaches the debugger to process `pid` (its PID inside the container) in `container`.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = debug, no_json)]
pub struct AttachToContainerPid {
    pub container: String,
    pub pid: u32,
}

/// Makes `name` the launch profile for `project` (picked in "Select Launch Profile").
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = workbench, no_json)]
pub struct UseLaunchProfile {
    pub project: String,
    pub name: String,
}

/// Creates something from a template (picked in New Project / New Item): `template` is a `dotnet new`
/// short name, or `csharp:<keyword>` for ByteForge's own C# items; `dir` is the folder for an item.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = dotnet, no_json)]
pub struct UseTemplate {
    pub template: String,
    pub title: String,
    pub project: bool,
    pub dir: String,
}

/// Makes `shell` the terminal's default (empty: the login shell, `$SHELL`) and opens a terminal with it.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = terminal, no_json)]
pub struct SetDefaultShell {
    pub shell: String,
}

/// Formats the current document with this formatter (a module formatter id, or "csharp-ls") and makes it
/// the default.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = workbench, no_json)]
pub struct UseFormatter {
    pub id: String,
}

/// Executes a command contributed by an out-of-process module.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = module, no_json)]
pub struct RunModuleCommand {
    pub id: String,
}

pub struct CommandInfo {
    pub id: String,
    pub title: String,
    pub category: &'static str,
    pub action: Box<dyn Action>,
}

impl CommandInfo {
    pub fn display_title(&self) -> String {
        if self.category.is_empty() { self.title.clone() } else { format!("{}: {}", self.category, self.title) }
    }
}

/// (id, title, category, action, default key)
type Builtin = (&'static str, &'static str, &'static str, Box<dyn Action>, Option<&'static str>);

fn builtin() -> Vec<Builtin> {
    vec![
        ("file.newFile", "New File", "File", Box::new(NewFile), Some("ctrl-n")),
        ("file.openFile", "Open File…", "File", Box::new(OpenFile), Some("ctrl-o")),
        ("file.openFolder", "Open Folder…", "File", Box::new(OpenFolder), Some("ctrl-shift-o")),
        ("file.save", "Save", "File", Box::new(Save), Some("ctrl-s")),
        ("file.saveAs", "Save As…", "File", Box::new(SaveAs), Some("ctrl-shift-s")),
        ("file.saveAll", "Save All", "File", Box::new(SaveAll), Some("ctrl-alt-s")),
        ("file.close", "Close Editor", "File", Box::new(CloseEditor), Some("ctrl-w")),
        ("file.closeAll", "Close All Editors", "File", Box::new(CloseAllEditors), Some("ctrl-shift-w")),
        ("app.exit", "Exit", "File", Box::new(Quit), Some("ctrl-q")),
        ("view.commandPalette", "Command Palette", "View", Box::new(CommandPalette), Some("ctrl-shift-p")),
        ("view.quickOpen", "Go to File…", "Go", Box::new(QuickOpen), Some("ctrl-p")),
        ("edit.goToLine", "Go to Line…", "Go", Box::new(GoToLine), Some("ctrl-g")),
        ("view.toggleSideBar", "Toggle Side Bar", "View", Box::new(ToggleSideBar), Some("ctrl-b")),
        ("view.resetActivityBar", "Reset Activity Bar (Order and Hidden Items)", "View", Box::new(ResetActivityBarOrder), None),
        ("view.togglePanel", "Toggle Panel", "View", Box::new(TogglePanel), Some("ctrl-j")),
        ("view.explorer", "Explorer", "View", Box::new(ShowExplorer), Some("ctrl-shift-e")),
        ("view.search", "Search", "View", Box::new(ShowSearch), Some("ctrl-shift-f")),
        ("view.problems", "Problems", "View", Box::new(ShowProblems), Some("ctrl-shift-m")),
        ("view.output", "Output", "View", Box::new(ShowOutput), Some("ctrl-shift-u")),
        ("view.modules", "Modules", "View", Box::new(ShowModules), None),
        ("view.scm", "Source Control", "View", Box::new(ShowSourceControl), Some("ctrl-shift-g")),
        ("git.refresh", "Refresh", "Git", Box::new(GitRefresh), None),
        ("git.commit", "Commit", "Git", Box::new(GitCommit), None),
        ("git.stageAll", "Stage All Changes", "Git", Box::new(GitStageAll), None),
        ("git.unstageAll", "Unstage All Changes", "Git", Box::new(GitUnstageAll), None),
        ("git.pull", "Pull", "Git", Box::new(GitPull), None),
        ("git.push", "Push", "Git", Box::new(GitPush), None),
        ("git.fetch", "Fetch", "Git", Box::new(GitFetch), None),
        ("git.sync", "Sync (Pull, then Push)", "Git", Box::new(GitSync), None),
        ("git.checkout", "Checkout to…", "Git", Box::new(GitCheckout), None),
        ("git.createBranch", "Create Branch…", "Git", Box::new(GitCreateBranch), None),
        ("git.init", "Initialize Repository", "Git", Box::new(GitInit), None),
        ("git.openChanges", "Open Changes (Diff of Current File)", "Git", Box::new(GitOpenChanges), None),
        ("git.fileHistory", "File History", "Git", Box::new(GitFileHistory), None),
        ("git.showLineCommit", "Show Commit of Line", "Git", Box::new(ShowLineCommit), None),
        ("git.toggleBlame", "Toggle Line Blame", "Git", Box::new(GitToggleBlame), None),
        ("merge.acceptCurrent", "Accept Current Change", "Merge Conflict", Box::new(AcceptCurrentChange), None),
        ("merge.acceptIncoming", "Accept Incoming Change", "Merge Conflict", Box::new(AcceptIncomingChange), None),
        ("merge.acceptBoth", "Accept Both Changes", "Merge Conflict", Box::new(AcceptBothChanges), None),
        ("merge.next", "Next Conflict", "Merge Conflict", Box::new(NextConflict), None),
        ("merge.previous", "Previous Conflict", "Merge Conflict", Box::new(PreviousConflict), None),
        ("view.nextEditor", "Next Editor", "View", Box::new(NextEditor), Some("ctrl-tab")),
        ("view.previousEditor", "Previous Editor", "View", Box::new(PreviousEditor), Some("ctrl-shift-tab")),
        ("view.splitEditor", "Split Editor (Move to Other Group)", "View", Box::new(SplitEditor), Some("ctrl-\\")),
        ("view.openInOtherGroup", "Open Current File in Other Group", "View", Box::new(OpenInOtherGroup), None),
        ("view.focusFirstGroup", "Focus First Editor Group", "View", Box::new(FocusFirstGroup), Some("ctrl-1")),
        ("view.focusSecondGroup", "Focus Second Editor Group", "View", Box::new(FocusSecondGroup), Some("ctrl-2")),
        ("markdown.openPreview", "Open Preview", "Markdown", Box::new(MarkdownOpenPreview), Some("ctrl-shift-v")),
        ("markdown.openPreviewToSide", "Open Preview to the Side", "Markdown", Box::new(MarkdownOpenPreviewToSide), None),
        ("theme.select", "Color Theme", "Preferences", Box::new(SelectTheme), Some("ctrl-k")),
        ("theme.customize", "Customize Current Theme", "Preferences", Box::new(CustomizeTheme), None),
        ("theme.edit", "Open Current Theme File", "Preferences", Box::new(EditTheme), None),
        ("theme.reload", "Reload Theme", "Preferences", Box::new(ReloadTheme), None),
        ("settings.open", "Open Settings (JSON)", "Preferences", Box::new(OpenSettings), Some("ctrl-,")),
        ("app.about", "About ByteForge", "Help", Box::new(About), None),
        ("app.checkForUpdates", "Check for Updates…", "Help", Box::new(CheckForUpdates), None),
        ("dotnet.build", "Build", "Run", Box::new(Build), Some("ctrl-shift-b")),
        ("dotnet.run", "Run Without Debugging", "Run", Box::new(Run), Some("ctrl-f5")),
        ("dotnet.watch", "Run with Watch (Hot Reload)", "Run", Box::new(RunWithWatch), Some("alt-f5")),
        ("dotnet.selectLaunchProfile", "Select Launch Profile…", "Run", Box::new(SelectLaunchProfile), None),
        ("dotnet.newProject", "New Project…", "File", Box::new(NewProject), None),
        ("dotnet.newItem", "New Item from Template…", "File", Box::new(NewItem), None),
        ("dotnet.userSecrets", "Manage User Secrets", ".NET", Box::new(ManageUserSecrets), None),
        ("ef.addMigration", "Add Migration…", "EF Core", Box::new(EfAddMigration), None),
        ("ef.removeMigration", "Remove Last Migration", "EF Core", Box::new(EfRemoveMigration), None),
        ("ef.updateDatabase", "Update Database", "EF Core", Box::new(EfUpdateDatabase), None),
        ("ef.updateDatabaseTo", "Update Database to Migration…", "EF Core", Box::new(EfUpdateDatabaseTo), None),
        ("ef.script", "Generate SQL Script", "EF Core", Box::new(EfGenerateScript), None),
        ("ef.dropDatabase", "Drop Database…", "EF Core", Box::new(EfDropDatabase), None),
        ("dotnet.test", "Test", "Run", Box::new(Test), None),
        ("dotnet.clean", "Clean", "Run", Box::new(Clean), None),
        ("dotnet.restore", "Restore", "Run", Box::new(Restore), None),
        ("dotnet.stop", "Stop", "Run", Box::new(Stop), Some("shift-f5")),
        ("debug.start", "Start Debugging / Continue", "Debug", Box::new(StartDebugging), Some("f5")),
        ("debug.restart", "Restart", "Debug", Box::new(RestartDebugging), Some("ctrl-shift-f5")),
        ("debug.pause", "Pause", "Debug", Box::new(Pause), Some("f6")),
        ("debug.stepOver", "Step Over", "Debug", Box::new(StepOver), Some("f10")),
        ("debug.stepInto", "Step Into", "Debug", Box::new(StepInto), Some("f11")),
        ("debug.stepOut", "Step Out", "Debug", Box::new(StepOut), Some("shift-f11")),
        ("debug.toggleBreakpoint", "Toggle Breakpoint", "Debug", Box::new(ToggleBreakpoint), Some("f9")),
        ("debug.removeAllBreakpoints", "Remove All Breakpoints", "Debug", Box::new(RemoveAllBreakpoints), None),
        ("debug.attach", "Attach to .NET Process…", "Debug", Box::new(AttachToProcess), None),
        ("debug.attachContainer", "Attach to .NET Process in Container…", "Debug", Box::new(AttachToContainer), None),
        ("view.debug", "Run and Debug", "View", Box::new(ShowDebug), Some("ctrl-shift-d")),
        ("view.debugConsole", "Debug Console", "View", Box::new(ShowDebugConsole), Some("ctrl-shift-y")),
        ("view.testing", "Testing", "View", Box::new(ShowTesting), None),
        ("view.solution", "Solution", "View", Box::new(ShowSolution), None),
        ("nuget.add", "Add Package…", "NuGet", Box::new(AddPackage), None),
        ("nuget.checkUpdates", "Check for Package Updates", "NuGet", Box::new(CheckPackageUpdates), None),
        ("test.runAll", "Run All Tests", "Test", Box::new(RunAllTests), None),
        ("test.rerunFailed", "Rerun Failed Tests", "Test", Box::new(RerunFailedTests), None),
        ("test.runAtCursor", "Run Test at Cursor", "Test", Box::new(RunTestAtCursor), None),
        ("test.debugAtCursor", "Debug Test at Cursor", "Test", Box::new(DebugTestAtCursor), None),
        ("test.refresh", "Find Tests Again", "Test", Box::new(RefreshTests), None),
        ("terminal.toggle", "Toggle Terminal", "Terminal", Box::new(ToggleTerminal), Some("ctrl-`")),
        ("terminal.new", "New Terminal", "Terminal", Box::new(NewTerminal), Some("ctrl-shift-`")),
        ("terminal.kill", "Kill Terminal", "Terminal", Box::new(KillTerminal), None),
        ("terminal.selectDefaultShell", "Select Default Shell…", "Terminal", Box::new(SelectDefaultShell), None),
        ("edit.toggleLineEnding", "Toggle Line Ending (LF/CRLF)", "Edit", Box::new(ToggleLineEnding), None),
        ("csharp.reload", "Restart C# Language Server", "C#", Box::new(ReloadCSharp), None),
        ("edit.undo", "Undo", "Edit", Box::new(Undo), None),
        ("edit.redo", "Redo", "Edit", Box::new(Redo), None),
        ("edit.find", "Find", "Edit", Box::new(Find), None),
        ("edit.replace", "Replace", "Edit", Box::new(Replace), None),
        ("edit.toggleComment", "Toggle Line Comment", "Edit", Box::new(ToggleComment), None),
        ("edit.duplicateLines", "Duplicate Lines", "Edit", Box::new(DuplicateLines), None),
        ("edit.addCursorAbove", "Add Cursor Above", "Selection", Box::new(AddCursorAbove), None),
        ("edit.addCursorBelow", "Add Cursor Below", "Selection", Box::new(AddCursorBelow), None),
        ("edit.addNextMatch", "Add Selection to Next Find Match", "Selection", Box::new(AddNextMatch), None),
        ("edit.selectAllMatches", "Select All Occurrences", "Selection", Box::new(SelectAllMatches), None),
        ("edit.goToDefinition", "Go to Definition", "Go", Box::new(GoToDefinition), None),
        ("edit.triggerSuggest", "Trigger Suggest", "Edit", Box::new(TriggerCompletion), None),
        ("edit.quickFix", "Quick Fix…", "Edit", Box::new(QuickFix), None),
        ("edit.findReferences", "Find All References", "Go", Box::new(FindReferences), None),
        ("edit.goToImplementation", "Go to Implementations", "Go", Box::new(GoToImplementation), None),
        ("edit.goToTypeDefinition", "Go to Type Definition", "Go", Box::new(GoToTypeDefinition), None),
        ("view.goToSymbol", "Go to Symbol in Editor…", "Go", Box::new(GoToSymbolInEditor), Some("alt-\\")),
        ("view.goToWorkspaceSymbol", "Go to Symbol in Workspace…", "Go", Box::new(GoToSymbolInWorkspace), Some("ctrl-t")),
        ("edit.rename", "Rename Symbol", "Edit", Box::new(RenameSymbol), None),
        ("refactor.menu", "Refactor…", "Refactor", Box::new(Refactor), Some("ctrl-shift-r")),
        ("refactor.extractMethod", "Extract Method", "Refactor", Box::new(ExtractMethod), Some("ctrl-r ctrl-m")),
        ("refactor.extractLocalFunction", "Extract Local Function", "Refactor", Box::new(ExtractLocalFunction), None),
        ("refactor.introduceVariable", "Introduce Local Variable", "Refactor", Box::new(IntroduceVariable), None),
        ("refactor.introduceConstant", "Introduce Constant", "Refactor", Box::new(IntroduceConstant), None),
        ("refactor.introduceField", "Introduce Field", "Refactor", Box::new(IntroduceField), None),
        ("refactor.introduceParameter", "Introduce Parameter", "Refactor", Box::new(IntroduceParameter), None),
        ("refactor.inline", "Inline", "Refactor", Box::new(InlineSymbol), None),
        ("refactor.extractInterface", "Extract Interface", "Refactor", Box::new(ExtractInterface), Some("ctrl-r ctrl-i")),
        ("refactor.extractBaseClass", "Extract Base Class", "Refactor", Box::new(ExtractBaseClass), None),
        ("refactor.moveTypeToFile", "Move Type to Matching File", "Refactor", Box::new(MoveTypeToFile), None),
        ("refactor.removeAndSortUsings", "Remove and Sort Usings", "Refactor", Box::new(RemoveAndSortUsings), Some("ctrl-r ctrl-g")),
        ("refactor.sortUsings", "Sort Usings", "Refactor", Box::new(SortUsings), None),
        ("edit.fold", "Fold", "View", Box::new(Fold), None),
        ("edit.unfold", "Unfold", "View", Box::new(Unfold), None),
        ("edit.foldAll", "Fold All", "View", Box::new(FoldAll), None),
        ("edit.unfoldAll", "Unfold All", "View", Box::new(UnfoldAll), None),
        ("edit.signatureHelp", "Trigger Parameter Hints", "Edit", Box::new(ShowSignatureHelp), None),
        ("diff.nextChange", "Next Change", "Diff", Box::new(DiffNextChange), None),
        ("diff.previousChange", "Previous Change", "Diff", Box::new(DiffPreviousChange), None),
        ("diff.toggleInline", "Toggle Inline / Side by Side", "Diff", Box::new(DiffToggleInline), None),
        ("edit.formatDocument", "Format Document", "Edit", Box::new(FormatDocument), Some("shift-alt-f")),
        ("edit.formatDocumentWith", "Format Document With…", "Edit", Box::new(FormatDocumentWith), None),
    ]
}

/// Commands known to the palette and menus. Module commands are appended at runtime.
#[derive(Default)]
pub struct Commands {
    pub list: Vec<CommandInfo>,
}

impl gpui::Global for Commands {}

impl Commands {
    pub fn get(&self, id: &str) -> Option<&CommandInfo> {
        self.list.iter().find(|c| c.id == id)
    }
}

const EDITOR: &str = "Editor";
const INPUT: &str = "TextInput";
const TERMINAL: &str = "Terminal";
const EXPLORER: &str = "Explorer";

/// Ctrl+letter keys that belong to the shell inside the terminal (line editing, history search, ^C, …),
/// even though some are IDE shortcuts elsewhere. Ctrl+P and Ctrl+J stay with the IDE, like in VS Code.
const SHELL_KEYS: &str = "abcdefghklmnoqrstuvwxyz";

/// Installs default bindings, then user overrides from settings ("keybindings": { "view.quickOpen": "ctrl-e" }).
pub fn install(cx: &mut App, overrides: &HashMap<String, String>) {
    let mut commands = Commands::default();
    let mut bindings = Vec::new();
    for (id, title, category, action, key) in builtin() {
        // User overrides are taken as written ("secondary-" means Cmd on macOS, Ctrl elsewhere).
        let key = match overrides.get(id) {
            Some(k) if k.trim().is_empty() => None,
            Some(k) => Some(k.clone()),
            None => key.map(platform_keys),
        };
        if let Some(key) = key {
            let mapper = cx.keyboard_mapper().clone();
            match KeyBinding::load(&key, action.boxed_clone(), None, false, None, mapper.as_ref()) {
                Ok(binding) => bindings.push(binding),
                Err(e) => log::warn!("invalid keybinding '{key}' for {id}: {e}"),
            }
        }
        commands.list.push(CommandInfo { id: id.into(), title: title.into(), category, action });
    }
    // F1 first so Ctrl+Shift+P stays the displayed (highest-precedence) binding.
    cx.bind_keys([kb("f1", CommandPalette, None)]);
    cx.bind_keys(bindings);

    let e = Some(EDITOR);
    cx.bind_keys([
        kb("left", MoveLeft, e),
        kb("right", MoveRight, e),
        kb("up", MoveUp, e),
        kb("down", MoveDown, e),
        kb("shift-left", SelectLeft, e),
        kb("shift-right", SelectRight, e),
        kb("shift-up", SelectUp, e),
        kb("shift-down", SelectDown, e),
        kb("ctrl-left", MoveWordLeft, e),
        kb("ctrl-right", MoveWordRight, e),
        kb("ctrl-shift-left", SelectWordLeft, e),
        kb("ctrl-shift-right", SelectWordRight, e),
        kb("home", MoveHome, e),
        kb("end", MoveEnd, e),
        kb("shift-home", SelectHome, e),
        kb("shift-end", SelectEnd, e),
        kb("ctrl-home", MoveDocStart, e),
        kb("ctrl-end", MoveDocEnd, e),
        kb("ctrl-shift-home", SelectDocStart, e),
        kb("ctrl-shift-end", SelectDocEnd, e),
        kb("pageup", PageUp, e),
        kb("pagedown", PageDown, e),
        kb("shift-pageup", SelectPageUp, e),
        kb("shift-pagedown", SelectPageDown, e),
        kb("ctrl-up", ScrollLineUp, e),
        kb("ctrl-down", ScrollLineDown, e),
        kb("backspace", Backspace, e),
        kb("shift-backspace", Backspace, e),
        kb("ctrl-backspace", DeleteWordLeft, e),
        kb("delete", Delete, e),
        kb("ctrl-delete", DeleteWordRight, e),
        kb("enter", Newline, e),
        kb("tab", Tab, e),
        kb("shift-tab", Outdent, e),
        kb("ctrl-a", SelectAll, e),
        kb("ctrl-c", Copy, e),
        kb("ctrl-insert", Copy, e),
        kb("ctrl-x", Cut, e),
        kb("ctrl-v", Paste, e),
        kb("shift-insert", Paste, e),
        kb("ctrl-z", Undo, e),
        kb("ctrl-y", Redo, e),
        kb("ctrl-shift-z", Redo, e),
        kb("shift-alt-down", DuplicateLines, e),
        kb("ctrl-d", AddNextMatch, e),
        kb("ctrl-shift-l", SelectAllMatches, e),
        kb("ctrl-alt-up", AddCursorAbove, e),
        kb("ctrl-alt-down", AddCursorBelow, e),
        kb("ctrl-shift-k", DeleteLines, e),
        kb("alt-up", MoveLinesUp, e),
        kb("alt-down", MoveLinesDown, e),
        kb("ctrl-/", ToggleComment, e),
        kb("ctrl-#", ToggleComment, e),
        kb("ctrl-f", Find, e),
        kb("ctrl-h", Replace, e),
        kb("f3", FindNext, e),
        kb("shift-f3", FindPrevious, e),
        kb("ctrl-space", TriggerCompletion, e),
        kb("f12", GoToDefinition, e),
        kb("ctrl-.", QuickFix, e),
        kb("shift-f12", FindReferences, e),
        kb("ctrl-f12", GoToImplementation, e),
        kb("ctrl-shift-[", Fold, e),
        kb("ctrl-shift-]", Unfold, e),
        kb("f2", RenameSymbol, e),
        kb("ctrl-r ctrl-r", RenameSymbol, e),
        kb("ctrl-shift-space", ShowSignatureHelp, e),
        kb("escape", Cancel, e),
    ]);
    if cfg!(target_os = "macos") {
        // Cmd+Arrow goes to the line ends, like every macOS text view (Cmd+Up/Down come from ctrl-home/end).
        cx.bind_keys([
            KeyBinding::new("cmd-left", MoveHome, e),
            KeyBinding::new("cmd-right", MoveEnd, e),
            KeyBinding::new("cmd-shift-left", SelectHome, e),
            KeyBinding::new("cmd-shift-right", SelectEnd, e),
            KeyBinding::new("cmd-left", InputHome, Some(INPUT)),
            KeyBinding::new("cmd-right", InputEnd, Some(INPUT)),
        ]);
    }

    cx.bind_keys([kb("f2", RenameEntry, Some(EXPLORER))]);
    cx.bind_keys([kb("f7", DiffNextChange, Some("DiffView")), kb("shift-f7", DiffPreviousChange, Some("DiffView"))]);

    let term = Some(TERMINAL);
    let mut terminal_bindings = if cfg!(target_os = "macos") {
        // Cmd never reaches the shell on macOS, so plain Cmd+C/V can copy and paste.
        vec![KeyBinding::new("cmd-c", TerminalCopy, term), KeyBinding::new("cmd-v", TerminalPaste, term)]
    } else {
        vec![KeyBinding::new("ctrl-shift-c", TerminalCopy, term), KeyBinding::new("ctrl-shift-v", TerminalPaste, term), KeyBinding::new("shift-insert", TerminalPaste, term)]
    };
    for c in SHELL_KEYS.chars() {
        // NoAction hides the IDE binding here, so the key reaches the terminal's key handler.
        terminal_bindings.push(KeyBinding::new(&format!("ctrl-{c}"), gpui::NoAction, term));
    }
    cx.bind_keys(terminal_bindings);

    let i = Some(INPUT);
    cx.bind_keys([
        kb("enter", Confirm, i),
        kb("backspace", InputBackspace, i),
        kb("delete", InputDelete, i),
        kb("left", InputLeft, i),
        kb("right", InputRight, i),
        kb("shift-left", InputSelectLeft, i),
        kb("shift-right", InputSelectRight, i),
        kb("home", InputHome, i),
        kb("end", InputEnd, i),
        kb("ctrl-a", InputSelectAll, i),
        kb("ctrl-c", InputCopy, i),
        kb("ctrl-x", InputCut, i),
        kb("ctrl-v", InputPaste, i),
        kb("up", InputUp, i),
        kb("down", InputDown, i),
        kb("pageup", InputPageUp, i),
        kb("pagedown", InputPageDown, i),
        kb("escape", InputCancel, i),
    ]);

    cx.set_global(commands);
}

/// A built-in binding, translated for the platform (see [`platform_keys`]).
fn kb<A: gpui::Action>(keys: &str, action: A, context: Option<&str>) -> KeyBinding {
    KeyBinding::new(&platform_keys(keys), action, context)
}

/// Built-in bindings are written the Windows/Linux way ("ctrl-s"). On macOS they become their Mac
/// equivalents: Ctrl turns into Cmd, except for keys macOS reserves or where Mac editors differ.
pub fn platform_keys(keys: &str) -> String {
    if !cfg!(target_os = "macos") {
        return keys.to_owned();
    }
    keys.split_whitespace().map(mac_chord).collect::<Vec<_>>().join(" ")
}

fn mac_chord(chord: &str) -> String {
    let special = match chord {
        // Word and document movement follow the macOS text conventions.
        "ctrl-left" => "alt-left",
        "ctrl-right" => "alt-right",
        "ctrl-shift-left" => "alt-shift-left",
        "ctrl-shift-right" => "alt-shift-right",
        "ctrl-home" => "cmd-up",
        "ctrl-end" => "cmd-down",
        "ctrl-shift-home" => "cmd-shift-up",
        "ctrl-shift-end" => "cmd-shift-down",
        "ctrl-backspace" => "alt-backspace",
        "ctrl-delete" => "alt-delete",
        // Cmd+H hides the app and Cmd+Shift+[ / ] switch tabs on macOS.
        "ctrl-h" => "cmd-alt-f",
        "ctrl-shift-[" => "cmd-alt-[",
        "ctrl-shift-]" => "cmd-alt-]",
        // Cmd+Space is Spotlight and Cmd+Tab the app switcher; these stay on Ctrl, like in VS Code for macOS.
        "ctrl-space" | "ctrl-shift-space" | "ctrl-tab" | "ctrl-shift-tab" | "ctrl-`" | "ctrl-shift-`" | "ctrl-g" | "ctrl-f5" | "ctrl-up"
        | "ctrl-down" => chord,
        _ => "",
    };
    if !special.is_empty() {
        return special.to_owned();
    }
    chord.split('-').map(|part| if part == "ctrl" { "cmd" } else { part }).collect::<Vec<_>>().join("-")
}

/// A built-in binding as shown to the user: "ctrl-shift-p" → "Ctrl+Shift+P" (on macOS "⇧⌘P").
pub fn key_label(keys: &str) -> String {
    pretty_keys(&platform_keys(keys))
}

/// "ctrl-shift-p" → "Ctrl+Shift+P" (chords separated by spaces); macOS uses its symbols: "cmd-shift-p" → "⇧⌘P".
pub fn pretty_keys(text: &str) -> String {
    let mac = cfg!(target_os = "macos");
    text.split_whitespace()
        .map(|chord| {
            let mut parts: Vec<&str> = chord.split('-').collect();
            if mac {
                // Apple's order: ⌃⌥⇧⌘, then the key.
                let rank = |p: &str| match p {
                    "ctrl" => 0,
                    "alt" => 1,
                    "shift" => 2,
                    "cmd" | "super" | "win" | "secondary" => 3,
                    _ => 4,
                };
                parts.sort_by_key(|p| rank(p));
            }
            let labels: Vec<String> = parts
                .iter()
                .map(|part| match (*part, mac) {
                    ("ctrl", true) => "⌃".to_owned(),
                    ("alt", true) => "⌥".to_owned(),
                    ("shift", true) => "⇧".to_owned(),
                    ("cmd" | "super" | "win" | "secondary", true) => "⌘".to_owned(),
                    ("left", true) => "←".to_owned(),
                    ("right", true) => "→".to_owned(),
                    ("up", true) => "↑".to_owned(),
                    ("down", true) => "↓".to_owned(),
                    ("enter", true) => "↩".to_owned(),
                    ("backspace", true) => "⌫".to_owned(),
                    ("escape", true) => "⎋".to_owned(),
                    ("ctrl" | "secondary", false) => "Ctrl".to_owned(),
                    ("shift", false) => "Shift".to_owned(),
                    ("alt", false) => "Alt".to_owned(),
                    ("cmd" | "super" | "win", false) => "Super".to_owned(),
                    (other, _) if other.chars().count() == 1 => other.to_uppercase(),
                    (other, _) => {
                        let mut c = other.chars();
                        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                    }
                })
                .collect();
            labels.join(if mac { "" } else { "+" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::mac_chord;

    #[test]
    fn mac_bindings_use_cmd_except_where_macos_differs() {
        assert_eq!(mac_chord("ctrl-shift-p"), "cmd-shift-p");
        assert_eq!(mac_chord("ctrl-s"), "cmd-s");
        assert_eq!(mac_chord("ctrl-left"), "alt-left");
        assert_eq!(mac_chord("ctrl-home"), "cmd-up");
        assert_eq!(mac_chord("ctrl-h"), "cmd-alt-f");
        assert_eq!(mac_chord("ctrl-space"), "ctrl-space");
        assert_eq!(mac_chord("ctrl-`"), "ctrl-`");
        assert_eq!(mac_chord("shift-alt-f"), "shift-alt-f");
        assert_eq!(mac_chord("f12"), "f12");
    }
}
