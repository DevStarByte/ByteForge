using System.Text;
using ByteForge.Modules.Testing;

namespace ByteForge.Todos.Tests;

public sealed class TodosTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-todos-").FullName;

    public void Dispose()
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    string Write(string relative, string text)
    {
        var path = Path.Combine(dir, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
        return path;
    }

    /// <summary>A workspace with comments in scanned files and in places that are skipped.</summary>
    void Workspace()
    {
        Write("src/App.cs", "class A\n{\n    // TODO: rename this\n    void M() { } /* FIXME leaks */\n}\n");
        Write("src/view.html", "<!-- HACK: spacing -->\r\n<p>\r\n");
        Write("scripts/build.sh", "#!/bin/sh\n# BUG wrong path\necho hi # XXX\n");
        Write("Dockerfile", "FROM x\n# TODO pin the version\n");
        Write("notes.txt", "// TODO not scanned: .txt\n");
        Write("bin/Debug/Gen.cs", "// TODO skipped folder\n");
        Write(".hidden/a.cs", "// TODO hidden folder\n");
        Write("node_modules/x/a.js", "// TODO skipped\n");
        Write(".json", "// TODO a dot file has no extension\n");
        Write("src/Words.cs", "// TODOS isn't a tag, nor is MYTODO\nvar todo = 1; // todo lowercase\n");
    }

    [Fact]
    public void Finds_tagged_comments_in_scanned_files_only()
    {
        Workspace();
        var found = TodosModule.Sorted(TodosModule.Scan(dir)).Select(t => (Path.GetRelativePath(dir, t.Path).Replace('\\', '/'), t.Line, t.Column, t.Tag, t.Text)).ToList();
        Assert.Equal(
        [
            ("Dockerfile", 2, 3, "TODO", "pin the version"),
            ("src/App.cs", 3, 8, "TODO", "rename this"),
            ("scripts/build.sh", 2, 3, "BUG", "wrong path"),
            ("src/App.cs", 4, 21, "FIXME", "leaks"),
            ("src/view.html", 1, 6, "HACK", "spacing"),
            ("scripts/build.sh", 3, 11, "XXX", ""),
        ], found);
    }

    [Fact]
    public void Columns_count_characters_and_invalid_utf8_is_replaced()
    {
        var found = new List<Todo>();
        TodosModule.ScanFile("a.py", [.. Encoding.UTF8.GetBytes("x = '😀' # TODO: emoji\n"), 0xff, .. "# FIXME */\n"u8], found);
        Assert.Equal(new Todo("a.py", 1, 11, "TODO", "emoji"), found[0]);
        Assert.Equal(new Todo("a.py", 2, 4, "FIXME", ""), found[1]);
    }

    [Fact]
    public void Splits_extensions_like_python()
    {
        Assert.Equal(".cs", TodosModule.SplitExtension("a.b.cs"));
        Assert.Equal("", TodosModule.SplitExtension(".bashrc"));
        Assert.Equal("", TodosModule.SplitExtension("..json"));
        Assert.Equal("", TodosModule.SplitExtension("Makefile"));
    }

    [Fact]
    public async Task Status_and_list_through_the_protocol()
    {
        Workspace();
        await using var module = ModuleHarness.Start(new TodosModule(), Path.Combine(dir, "modules", "todos"));
        await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "todos.list" && m["category"]!.GetValue<string>() == "TODOs");
        await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "todos.refresh");
        module.Activate(dir);
        var status = await module.Next("status");
        Assert.Equal("todos.count", status["id"]!.GetValue<string>());
        Assert.Equal("☐ 6 (2 FIXME)", status["text"]!.GetValue<string>());
        Assert.Equal("todos.list", status["command"]!.GetValue<string>());
        Assert.Equal("6 TODO comments in the workspace. Click to list them.", status["tooltip"]!.GetValue<string>());

        module.Send(new() { ["type"] = "executeCommand", ["id"] = "todos.list" });
        await module.Next("log", m => m["text"]!.GetValue<string>() == "6 TODO comments:");
        var first = await module.Next("log");
        Assert.Equal("Dockerfile:2:3: TODO pin the version", first["text"]!.GetValue<string>());
        var second = await module.Next("log");
        Assert.Equal($"src{Path.DirectorySeparatorChar}App.cs:3:8: TODO rename this", second["text"]!.GetValue<string>());
        await module.Next("showView", m => m["id"]!.GetValue<string>() == "output");
    }

    [Fact]
    public async Task No_workspace_hides_the_count_and_an_empty_one_says_so()
    {
        await using (var module = ModuleHarness.Start(new TodosModule(), Path.Combine(dir, "modules", "todos")))
        {
            module.Activate();
            Assert.Equal("", (await module.Next("status"))["text"]!.GetValue<string>());
        }
        var empty = Directory.CreateDirectory(Path.Combine(dir, "empty")).FullName;
        await using (var module = ModuleHarness.Start(new TodosModule(), Path.Combine(dir, "modules", "todos")))
        {
            module.Activate(empty);
            Assert.Equal("☐ 0", (await module.Next("status"))["text"]!.GetValue<string>());
            module.Send(new() { ["type"] = "executeCommand", ["id"] = "todos.list" });
            await module.Next("log", m => m["text"]!.GetValue<string>() == "No TODO comments in the workspace.");
            await module.Next("showView");
        }
    }

    [Fact]
    public async Task Saving_a_file_refreshes_the_count()
    {
        var file = Write("a.cs", "// TODO one\n");
        await using var module = ModuleHarness.Start(new TodosModule(), Path.Combine(dir, "modules", "todos"));
        module.Activate(dir);
        Assert.Equal("☐ 1", (await module.Next("status"))["text"]!.GetValue<string>());
        File.AppendAllText(file, "// FIXME two\n");
        module.Send(new() { ["type"] = "event", ["name"] = "fileSaved", ["path"] = file });
        Assert.Equal("☐ 2 (1 FIXME)", (await module.Next("status"))["text"]!.GetValue<string>());
    }
}
