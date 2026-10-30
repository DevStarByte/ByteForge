//! Actions, the command registry (what the command palette and menus list) and key bindings.

use std::collections::HashMap;

use gpui::{Action, App, KeyBinding, actions};

actions!(
    workbench,
    [
        NewFile, OpenFile, OpenFolder, Save, SaveAs, SaveAll, CloseEditor, CloseAllEditors, Quit, CommandPalette, QuickOpen, GoToLine,
        ToggleSideBar, TogglePanel, ShowExplorer, ShowSearch, ShowProblems, ShowOutput, ShowModules, NextEditor, PreviousEditor,
        SelectTheme, CustomizeTheme, EditTheme, ReloadTheme, OpenSettings, About, Build, Run, Test, Clean, Restore, Stop,
        ToggleLineEnding, ReloadCSharp, ShowSourceControl, GitRefresh, GitCommit, GitStageAll, GitUnstageAll, GitPull, GitPush,
        GitFetch, GitSync, GitCheckout, GitCreateBranch, GitInit, GitOpenChanges
    ]
);

actions!(
    editor,
    [
        MoveLeft, MoveRight, MoveUp, MoveDown, SelectLeft, SelectRight, SelectUp, SelectDown, MoveWordLeft, MoveWordRight,
        SelectWordLeft, SelectWordRight, MoveHome, MoveEnd, SelectHome, SelectEnd, MoveDocStart, MoveDocEnd, SelectDocStart,
        SelectDocEnd, PageUp, PageDown, SelectPageUp, SelectPageDown, ScrollLineUp, ScrollLineDown, Backspace, DeleteWordLeft,
        Delete, DeleteWordRight, Newline, Tab, Outdent, SelectAll, Copy, Cut, Paste, Undo, Redo, DuplicateLines, DeleteLines,
        MoveLinesUp, MoveLinesDown, ToggleComment, Find, Replace, FindNext, FindPrevious, TriggerCompletion, GoToDefinition,
        Cancel
    ]
);

actions!(input, [Confirm, InputBackspace, InputDelete, InputLeft, InputRight, InputSelectLeft, InputSelectRight, InputHome, InputEnd, InputSelectAll, InputCopy, InputCut, InputPaste, InputUp, InputDown, InputCancel, InputPageUp, InputPageDown]);

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
fn builtin() -> Vec<(&'static str, &'static str, &'static str, Box<dyn Action>, Option<&'static str>)> {
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
        ("view.nextEditor", "Next Editor", "View", Box::new(NextEditor), Some("ctrl-tab")),
        ("view.previousEditor", "Previous Editor", "View", Box::new(PreviousEditor), Some("ctrl-shift-tab")),
        ("theme.select", "Color Theme", "Preferences", Box::new(SelectTheme), Some("ctrl-k")),
        ("theme.customize", "Customize Current Theme", "Preferences", Box::new(CustomizeTheme), None),
        ("theme.edit", "Open Current Theme File", "Preferences", Box::new(EditTheme), None),
        ("theme.reload", "Reload Theme", "Preferences", Box::new(ReloadTheme), None),
        ("settings.open", "Open Settings (JSON)", "Preferences", Box::new(OpenSettings), Some("ctrl-,")),
        ("app.about", "About ByteForge", "Help", Box::new(About), None),
        ("dotnet.build", "Build", "Run", Box::new(Build), Some("ctrl-shift-b")),
        ("dotnet.run", "Run", "Run", Box::new(Run), Some("f5")),
        ("dotnet.test", "Test", "Run", Box::new(Test), None),
        ("dotnet.clean", "Clean", "Run", Box::new(Clean), None),
        ("dotnet.restore", "Restore", "Run", Box::new(Restore), None),
        ("dotnet.stop", "Stop", "Run", Box::new(Stop), Some("shift-f5")),
        ("edit.toggleLineEnding", "Toggle Line Ending (LF/CRLF)", "Edit", Box::new(ToggleLineEnding), None),
        ("csharp.reload", "Restart C# Language Server", "C#", Box::new(ReloadCSharp), None),
        ("edit.undo", "Undo", "Edit", Box::new(Undo), None),
        ("edit.redo", "Redo", "Edit", Box::new(Redo), None),
        ("edit.find", "Find", "Edit", Box::new(Find), None),
        ("edit.replace", "Replace", "Edit", Box::new(Replace), None),
        ("edit.toggleComment", "Toggle Line Comment", "Edit", Box::new(ToggleComment), None),
        ("edit.duplicateLines", "Duplicate Lines", "Edit", Box::new(DuplicateLines), None),
        ("edit.goToDefinition", "Go to Definition", "Go", Box::new(GoToDefinition), None),
        ("edit.triggerSuggest", "Trigger Suggest", "Edit", Box::new(TriggerCompletion), None),
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

/// Installs default bindings, then user overrides from settings ("keybindings": { "view.quickOpen": "ctrl-e" }).
pub fn install(cx: &mut App, overrides: &HashMap<String, String>) {
    let mut commands = Commands::default();
    let mut bindings = Vec::new();
    for (id, title, category, action, key) in builtin() {
        let key = match overrides.get(id) {
            Some(k) if k.trim().is_empty() => None,
            Some(k) => Some(k.as_str()),
            None => key,
        };
        if let Some(key) = key {
            let mapper = cx.keyboard_mapper().clone();
            match KeyBinding::load(key, action.boxed_clone(), None, false, None, mapper.as_ref()) {
                Ok(binding) => bindings.push(binding),
                Err(e) => log::warn!("invalid keybinding '{key}' for {id}: {e}"),
            }
        }
        commands.list.push(CommandInfo { id: id.into(), title: title.into(), category, action });
    }
    // F1 first so Ctrl+Shift+P stays the displayed (highest-precedence) binding.
    cx.bind_keys([KeyBinding::new("f1", CommandPalette, None)]);
    cx.bind_keys(bindings);

    let e = Some(EDITOR);
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, e),
        KeyBinding::new("right", MoveRight, e),
        KeyBinding::new("up", MoveUp, e),
        KeyBinding::new("down", MoveDown, e),
        KeyBinding::new("shift-left", SelectLeft, e),
        KeyBinding::new("shift-right", SelectRight, e),
        KeyBinding::new("shift-up", SelectUp, e),
        KeyBinding::new("shift-down", SelectDown, e),
        KeyBinding::new("ctrl-left", MoveWordLeft, e),
        KeyBinding::new("ctrl-right", MoveWordRight, e),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, e),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, e),
        KeyBinding::new("home", MoveHome, e),
        KeyBinding::new("end", MoveEnd, e),
        KeyBinding::new("shift-home", SelectHome, e),
        KeyBinding::new("shift-end", SelectEnd, e),
        KeyBinding::new("ctrl-home", MoveDocStart, e),
        KeyBinding::new("ctrl-end", MoveDocEnd, e),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, e),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, e),
        KeyBinding::new("pageup", PageUp, e),
        KeyBinding::new("pagedown", PageDown, e),
        KeyBinding::new("shift-pageup", SelectPageUp, e),
        KeyBinding::new("shift-pagedown", SelectPageDown, e),
        KeyBinding::new("ctrl-up", ScrollLineUp, e),
        KeyBinding::new("ctrl-down", ScrollLineDown, e),
        KeyBinding::new("backspace", Backspace, e),
        KeyBinding::new("shift-backspace", Backspace, e),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, e),
        KeyBinding::new("delete", Delete, e),
        KeyBinding::new("ctrl-delete", DeleteWordRight, e),
        KeyBinding::new("enter", Newline, e),
        KeyBinding::new("tab", Tab, e),
        KeyBinding::new("shift-tab", Outdent, e),
        KeyBinding::new("ctrl-a", SelectAll, e),
        KeyBinding::new("ctrl-c", Copy, e),
        KeyBinding::new("ctrl-insert", Copy, e),
        KeyBinding::new("ctrl-x", Cut, e),
        KeyBinding::new("ctrl-v", Paste, e),
        KeyBinding::new("shift-insert", Paste, e),
        KeyBinding::new("ctrl-z", Undo, e),
        KeyBinding::new("ctrl-y", Redo, e),
        KeyBinding::new("ctrl-shift-z", Redo, e),
        KeyBinding::new("ctrl-d", DuplicateLines, e),
        KeyBinding::new("ctrl-shift-k", DeleteLines, e),
        KeyBinding::new("alt-up", MoveLinesUp, e),
        KeyBinding::new("alt-down", MoveLinesDown, e),
        KeyBinding::new("ctrl-/", ToggleComment, e),
        KeyBinding::new("ctrl-#", ToggleComment, e),
        KeyBinding::new("ctrl-f", Find, e),
        KeyBinding::new("ctrl-h", Replace, e),
        KeyBinding::new("f3", FindNext, e),
        KeyBinding::new("shift-f3", FindPrevious, e),
        KeyBinding::new("ctrl-space", TriggerCompletion, e),
        KeyBinding::new("f12", GoToDefinition, e),
        KeyBinding::new("escape", Cancel, e),
    ]);

    let i = Some(INPUT);
    cx.bind_keys([
        KeyBinding::new("enter", Confirm, i),
        KeyBinding::new("backspace", InputBackspace, i),
        KeyBinding::new("delete", InputDelete, i),
        KeyBinding::new("left", InputLeft, i),
        KeyBinding::new("right", InputRight, i),
        KeyBinding::new("shift-left", InputSelectLeft, i),
        KeyBinding::new("shift-right", InputSelectRight, i),
        KeyBinding::new("home", InputHome, i),
        KeyBinding::new("end", InputEnd, i),
        KeyBinding::new("ctrl-a", InputSelectAll, i),
        KeyBinding::new("ctrl-c", InputCopy, i),
        KeyBinding::new("ctrl-x", InputCut, i),
        KeyBinding::new("ctrl-v", InputPaste, i),
        KeyBinding::new("up", InputUp, i),
        KeyBinding::new("down", InputDown, i),
        KeyBinding::new("pageup", InputPageUp, i),
        KeyBinding::new("pagedown", InputPageDown, i),
        KeyBinding::new("escape", InputCancel, i),
    ]);

    cx.set_global(commands);
}

/// "ctrl-shift-p" → "Ctrl+Shift+P" (chords separated by spaces).
pub fn pretty_keys(text: &str) -> String {
    text.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
                .map(|part| match part {
                    "ctrl" => "Ctrl".to_owned(),
                    "shift" => "Shift".to_owned(),
                    "alt" => "Alt".to_owned(),
                    "cmd" | "super" | "win" => "Super".to_owned(),
                    other if other.chars().count() == 1 => other.to_uppercase(),
                    other => {
                        let mut c = other.chars();
                        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}