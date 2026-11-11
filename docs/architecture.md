# Architecture

```
crates/
  bf-text     rope buffer, undo, and the editing model (caret, selection, every editing command); UI-free and tested
  bf-syntax   tree-sitter highlighting (C#, JSON, Razor, XML, YAML) and modules' regex languages, emitting theme token names; incremental and viewport-ranged
  bf-lsp      LSP client (std threads + flume channels, no async runtime); tested live against csharp-ls
  bf-dap      Debug Adapter Protocol client in the same style; tested live against netcoredbg
  bf-core     paths, settings, themes (inheritance, hot reload), workspace index/watcher, search, dotnet runner,
              debug helpers (launch profiles, target assembly, build diagnostics), module host
  byteforge   the GPUI app: workbench, editor element, palette, explorer, search, theme → GPUI mapping, icons
modules/      bundled modules (CSharpier is preinstalled; Aspire, Azure and SQL come from the registry)
themes/       built-in themes (byteforge-dark.json documents every key)
packaging/    Linux install script, desktop entry, Arch PKGBUILD for local builds
              (the repo's PKGBUILD and build workflow live on the `packages` branch)
```

## Performance

- Edits are O(log n) in the rope.
- Highlighting runs on background threads over a window around the viewport (about 3 ms for a 32 KB window of
  a 60k-line file) and is applied only if the document hasn't changed in the meantime. Existing colors are
  shifted while you type, so nothing flickers.
- Only visible lines are shaped, and shaped lines are cached by content and colors.
- csharp-ls runs out of process, so a slow project load never blocks the UI.

## Build notes

- `libc` is pinned to 0.2.189 in `Cargo.lock` because newer versions removed `ENOATTR`, which `xattr` 0.2 (pulled
  in by GPUI) still needs.
