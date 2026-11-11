# ByteForge tasks: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. (This replaces ROADMAP.md; everything that was
done there is in the README and the git history.)

## 1. Make what's there trustworthy

- [ ] Dogfooding: use ByteForge for real work and fix what turns up. Built but never clicked through: the
      Refactor menu, diff editor, blame, merge conflicts, breadcrumbs, tab dragging, one file in both groups,
      the User Secrets page, live test progress.
- [ ] Branch picker: clicking a branch doesn't switch to it (needs a repro in the app).
- [ ] Use the Roslyn language server (the one VS Code's C# extension uses) instead of, or next to,
      csharp-ls: more refactorings (Change Signature), Razor IntelliSense.

## 2. Everyday features

- [ ] Search: regex and whole-word options in Find and in Search across files.
- [ ] Replace across files (Search view), with a preview of each match.
- [ ] Debugger: conditional breakpoints, logpoints, hit counts.
- [ ] Debugger: watch expressions, and a variable's value on hover while paused.
- [ ] Word wrap.
- [ ] Auto save.
- [ ] Settings screen (today only settings.json).

## 3. Left over from the roadmap

- [ ] EF Core: show which migrations are applied (needs the database), and `dotnet ef dbcontext scaffold`.
- [ ] Diff editor: stage or revert one change, collapse unchanged regions, edit the working-tree side.
- [ ] Blame for every line in the gutter; a three-way merge editor.
- [ ] F5 in a container: build, start and debug the app's container.

Not possible with today's tools: Change Signature with csharp-ls (needs a dialog it doesn't have; see
the Roslyn server above), and debugging in Alpine (musl) images (netcoredbg has no musl release).

## 4. Later

- [ ] Code coverage in the gutter.
- [ ] Git stash.
- [ ] Sticky scroll (the current type and member pinned at the top of the editor).
- [ ] An AI assistant panel.
