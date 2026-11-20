"""Tests for the Git Hooks module: python3 -m unittest discover -s modules/git-hooks -p 'test_*.py' -v"""

import io
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import git_hooks as gh  # noqa: E402


def capture(fn, *args):
    out = io.StringIO()
    with redirect_stdout(out):
        fn(*args)
    return [json.loads(line) for line in out.getvalue().splitlines()]


class RepoTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "t@example.invalid")
        self.git("config", "user.name", "T")
        self.module = gh.GitHooks()
        self.module.root = self.root
        self.module.scan()

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *args, check=True):
        return subprocess.run(["git", *args], cwd=self.root, capture_output=True, text=True, check=check)

    def tree(self):
        return capture(self.module.refresh)[-1]

    def pick(self, kind, value):
        request = next(r for r, (k, _) in self.module.pending.items() if k == kind)
        return capture(self.module.handle, {"type": "quickPickResult", "requestId": request, "value": value})


class HooksTests(RepoTest):
    def test_samples_are_left_out(self):
        tree = self.tree()
        self.assertEqual(tree["items"][0]["count"], 0)
        self.assertEqual(tree["items"][0]["label"], "Hooks in .git/hooks")

    def test_new_hook_from_template_works_with_git(self):
        capture(self.module.new_hook)
        index = next(i for i, t in enumerate(gh.TEMPLATES) if t[0] == "commit-msg")
        messages = self.pick("new", str(index))
        path = os.path.join(self.root, ".git", "hooks", "commit-msg")
        self.assertIn({"type": "openFile", "path": path}, messages)
        self.assertTrue(gh.is_executable(path))
        with open(os.path.join(self.root, "a.txt"), "w") as f:
            f.write("a")
        self.git("add", "a.txt")
        bad = self.git("commit", "-q", "-m", "did stuff", check=False)
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("Conventional Commits", bad.stderr)
        self.assertEqual(self.git("commit", "-q", "-m", "feat: stuff", check=False).returncode, 0)

    def test_turn_off_and_on(self):
        path = gh.write_hook(os.path.join(self.root, ".git", "hooks"), "pre-commit", "exit 1\n")
        hook = self.module.find("pre-commit")
        self.assertTrue(hook["active"])
        capture(self.module.tree_action, "hook:pre-commit", "toggle")
        self.assertTrue(os.path.exists(path + gh.OFF))
        self.assertFalse(os.path.exists(path))
        row = self.tree()["items"][0]["children"][0]
        self.assertEqual(row["description"], "off")
        self.assertIn("Turn On", [m.get("label") for m in row["menu"]])
        with open(os.path.join(self.root, "b.txt"), "w") as f:
            f.write("b")
        self.git("add", "b.txt")
        self.assertEqual(self.git("commit", "-q", "-m", "x", check=False).returncode, 0)  # git ignores .off
        capture(self.module.tree_action, "hook:pre-commit", "toggle")
        self.assertTrue(os.path.exists(path))

    def test_not_executable_is_marked(self):
        if os.name == "nt":
            self.skipTest("no executable bit on Windows")
        path = os.path.join(self.root, ".git", "hooks", "pre-push")
        with open(path, "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        row = self.tree()["items"][0]["children"][0]
        self.assertIn("not executable", row["description"])
        capture(self.module.tree_action, "hook:pre-push", "chmod")
        self.assertTrue(gh.is_executable(path))

    def test_core_hooks_path(self):
        self.git("config", "core.hooksPath", ".husky")
        self.module.scan()
        self.assertEqual(self.module.folder, os.path.join(self.root, ".husky"))
        gh.write_hook(self.module.folder, "pre-commit", "exit 0\n")
        tree = self.tree()
        self.assertEqual(tree["items"][0]["label"], "Hooks in .husky (core.hooksPath)")
        self.assertEqual(tree["items"][0]["children"][0]["label"], "pre-commit")

    def test_run_command_passes_git_arguments(self):
        folder = os.path.join(self.root, ".git", "hooks")
        gh.write_hook(folder, "commit-msg", 'echo "$1"\n')
        gh.write_hook(folder, "pre-push", 'echo "$1 $2"\n')
        self.git("remote", "add", "origin", "https://github.com/o/r.git")
        messages = capture(self.module.tree_action, "hook:commit-msg", "run")
        command = messages[0]["command"]
        self.assertRegex(command, r"commit-msg.* .*byteforge-commit-msg-")
        out = subprocess.run(command, shell=True, capture_output=True, text=True)
        self.assertEqual(out.returncode, 0)
        with open(out.stdout.strip()) as f:
            self.assertIn("feat:", f.read())
        command = capture(self.module.tree_action, "hook:pre-push", "run")[0]["command"]
        out = subprocess.run(command, shell=True, capture_output=True, text=True)
        self.assertEqual(out.stdout.strip(), "origin https://github.com/o/r.git")

    def test_replace_asks_first(self):
        gh.write_hook(os.path.join(self.root, ".git", "hooks"), "pre-push", "exit 0\n")
        capture(self.module.new_hook)
        index = next(i for i, t in enumerate(gh.TEMPLATES) if t[0] == "pre-push")
        messages = self.pick("new", str(index))
        self.assertEqual(messages[0]["type"], "prompt")
        capture(self.module.handle, {"type": "promptResult", "requestId": messages[0]["requestId"], "index": 0})
        with open(os.path.join(self.root, ".git", "hooks", "pre-push")) as f:
            self.assertIn("dotnet test", f.read())

    def test_husky_tasks(self):
        os.makedirs(os.path.join(self.root, ".husky"))
        with open(os.path.join(self.root, ".husky", "task-runner.json"), "w") as f:
            f.write('{\n  // comment\n  "tasks": [\n'
                    '    {"name": "format", "group": "pre-commit", "command": "dotnet", "args": ["format", "--verify-no-changes"]},\n'
                    '    {"name": "lint", "command": "npm", "args": ["run", "lint"]},\n  ]\n}\n')
        husky = self.tree()["items"][1]
        self.assertEqual(husky["count"], 2)
        self.assertEqual([g["label"] for g in husky["children"]], ["pre-commit", "(no group: run by name)"])
        messages = capture(self.module.tree_action, "husky:0", "click")
        self.assertEqual(messages[0]["command"], "dotnet husky run --name format")

    def test_not_a_repository(self):
        with tempfile.TemporaryDirectory() as plain:
            module = gh.GitHooks()
            module.root = plain
            tree = capture(module.refresh)[-1]
            self.assertIn("isn't a git repository", tree["message"])


class TemplateTests(unittest.TestCase):
    def test_templates_are_valid_sh(self):
        for name, title, body in gh.TEMPLATES:
            result = subprocess.run(["sh", "-n"], input=body, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, f"{title}: {result.stderr}")
            self.assertTrue(name is None or name in gh.HOOKS)

    def test_conventional_commits_check(self):
        body = next(b for n, _, b in gh.TEMPLATES if n == "commit-msg")
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
            path = f.name
        try:
            for message, ok in (("feat(api): add export", True), ("fix!: breaking", True), ("update stuff", False)):
                with open(path, "w") as f:
                    f.write(message + "\n")
                result = subprocess.run(["sh", "-c", body, "hook", path], capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, ok, message)
        finally:
            os.remove(path)

    def test_manifest(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        self.assertEqual(manifest["views"][0]["id"], gh.VIEW)


if __name__ == "__main__":
    unittest.main()
