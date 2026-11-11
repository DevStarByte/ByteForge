# Settings and keybindings

**Preferences → Settings** (Ctrl+,) lists the settings with switches and choices. They live in
`~/.config/byteforge/settings.json` (**Preferences → Open Settings (JSON)**), which reloads live and keeps its
comments when the settings screen changes a value:

```jsonc
{
  "workbench.theme": "ByteForge Dark",
  "editor.tabSize": 4, "editor.insertSpaces": true, "editor.smoothScrolling": true, "editor.minimap": true,
  "files.exclude": ["bin", "obj", ".git", "node_modules", "target"],
  "csharp.languageServer": "",                         // custom path to csharp-ls (optional)
  "dotnet.project": "src/MyApp/MyApp.csproj",          // build/run/debug target (optional)
  "dotnet.launchProfile": "https",                     // launchSettings.json profile (default: first "Project" one; "Select Launch Profile…" overrides it per project)
  "debug.netcoredbgPath": "",                          // custom path to netcoredbg (optional)
  "debug.justMyCode": true, "debug.stopAtEntry": false, "debug.openBrowser": true,
  "debug.breakOnExceptions": ["user-unhandled"],       // or ["all"], or []
  "terminal.shell": "/usr/bin/fish", "terminal.args": [],  // default: your login shell ($SHELL); also set by
                                                     // "Terminal: Select Default Shell…" (palette or the panel button)
  "editor.wordWrap": "off",                            // "on": long lines continue on the next rows (Alt+Z)
  "files.autoSave": "off",                             // or "afterDelay", "onFocusChange", "onWindowChange"
  "files.autoSaveDelay": 1000,                         // ms of quiet before "afterDelay" saves (no formatting)
  "editor.formatOnSave": false, "editor.defaultFormatter": "csharpier",   // or "csharp-ls"
  "refactor.previewMultiFileChanges": true,            // ask (listing the files) before a rename or refactoring changes several files
  "refactor.renameGenerated": true,                    // after Extract Method / Introduce Local / Extract Interface, rename what it made up
  "git.blame.currentLine": true,                       // who last changed the caret's line, shown after it
  "explorer.autoReveal": true,                         // expand and scroll the tree to the file you switch to
  "update.checkAutomatically": true,                   // daily update check (default: on for Windows and macOS)
  "update.includePrereleases": false,                  // also offer betas
  "explorer.nativeIcons": true,                        // macOS: Finder's file and folder icons (false: the theme's)
  "keybindings": { "view.quickOpen": "ctrl-e", "theme.select": "" },   // "" unbinds
  "modules.disabled": ["me.hello"]
}
```

## Default shortcuts

On macOS, read Ctrl as ⌘ in the table below (⌘⇧P, ⌘S, ⌘+Click, …). The exceptions follow macOS
conventions: Ctrl+Space, Ctrl+Tab, Ctrl+G, Ctrl+F5 and Ctrl+\` stay on ⌃; Replace is ⌘⌥F; fold/unfold are
⌘⌥[ / ⌘⌥]; words move with ⌥←/→, line ends with ⌘←/→ and the document ends with ⌘↑/↓. In
`"keybindings"`, `secondary-` means ⌘ on macOS and Ctrl elsewhere (`"view.quickOpen": "secondary-e"`).

| Shortcut | Action | Shortcut | Action |
|---|---|---|---|
| Ctrl+Shift+P / F1 | Command palette | Ctrl+P | Go to file |
| Ctrl+Shift+O | Open folder | Ctrl+G | Go to line |
| Ctrl+S / Ctrl+Shift+S | Save / Save as | F12, Ctrl+Click | Go to definition |
| Ctrl+F / Ctrl+H | Find / Replace | Ctrl+Space | Completion |
| Ctrl+/ (Ctrl+# on DE) | Toggle comment | Alt+↑/↓ | Move lines |
| Ctrl+D | Add next occurrence (multi-cursor) | Ctrl+Shift+K | Delete lines |
| Alt+Click, Ctrl+Alt+↑/↓ | Add cursor | Shift+Alt+↓ | Duplicate lines |
| Ctrl+\\ | Split editor | Ctrl+1 / Ctrl+2 | Focus editor group |
| Ctrl+B / Ctrl+J | Toggle side bar / panel | Ctrl+Shift+E / F | Explorer / Search |
| Ctrl+Shift+B | Build | F5 | Start debugging / Continue |
| Ctrl+F5 | Run without debugging | Shift+F5 | Stop |
| Alt+F5 | Run with Watch (hot reload) | Shift+Alt+F5 | Restart Watch |
| F9 | Toggle breakpoint | F10 / F11 / Shift+F11 | Step over / into / out |
| F6 | Pause | Ctrl+Shift+F5 | Restart debugging |
| Ctrl+Shift+D | Run and Debug | Ctrl+Shift+Y | Debug Console |
| Ctrl+\` | Toggle terminal | Ctrl+Shift+\` | New terminal |
| Ctrl+. | Quick fix / refactor at the cursor | Shift+Alt+F | Format document |
| Ctrl+Shift+R | Refactor… (refactorings only) | F2, Ctrl+R Ctrl+R | Rename symbol |
| Ctrl+R Ctrl+M | Extract method | Ctrl+R Ctrl+I | Extract interface |
| Ctrl+R Ctrl+G | Remove and sort usings | F7 / Shift+F7 | Next / previous change (diff editor) |
| Ctrl+K | Color theme | Ctrl+Tab | Next editor |
| Ctrl+Shift+G | Source Control | Alt+\\ | Go to symbol in editor |
