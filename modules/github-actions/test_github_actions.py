"""Tests for the GitHub Actions module: python3 -m unittest discover -s modules/github-actions -p 'test_*.py' -v"""

import datetime
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

import github_actions as ga  # noqa: E402


def iso(minutes_ago):
    moment = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=minutes_ago)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def run(id, name="CI", branch="master", status="completed", conclusion="success", number=1, minutes_ago=10):
    return {"id": id, "name": name, "path": ".github/workflows/ci.yml", "run_number": number, "head_branch": branch,
            "head_sha": "abcdef1234", "event": "push", "status": status, "conclusion": conclusion,
            "display_title": f"Commit {id}", "created_at": iso(minutes_ago), "run_started_at": iso(minutes_ago),
            "updated_at": iso(minutes_ago - 3), "html_url": f"https://github.com/o/r/actions/runs/{id}",
            "actor": {"login": "dev"}}


def pull(number, ref, title, fork=False, draft=False):
    return {"number": number, "title": title, "draft": draft, "user": {"login": "dev"}, "labels": [],
            "head": {"ref": ref, "sha": f"sha{number}", "repo": {"full_name": "someone/r" if fork else "o/r"}},
            "base": {"ref": "master"}, "updated_at": iso(20), "html_url": f"https://github.com/o/r/pull/{number}"}


class FakeApi(ga.Api):
    def __init__(self, token=None):
        super().__init__(token)
        self.calls = []
        self.runs = [run(3, status="in_progress", conclusion=None, number=3, minutes_ago=1),
                     run(2, conclusion="failure", number=2, minutes_ago=30),
                     run(1, name="Release", branch="v1.0", number=1, minutes_ago=600)]
        self.pulls = [pull(12, "feature/x", "Add export"), pull(7, "master", "Fix typo", fork=True, draft=True)]
        self.issues = [{"number": 30, "title": "Crash on start!", "labels": [{"name": "bug"}], "assignees": [],
                        "user": {"login": "a"}, "updated_at": iso(5), "comments": 2, "html_url": "https://i30"},
                       {"number": 31, "title": "Dark mode", "labels": [], "assignees": [{"login": "dev"}],
                        "user": {"login": "b"}, "updated_at": iso(50), "comments": 0, "html_url": "https://i31"},
                       {"number": 12, "title": "Add export", "pull_request": {}, "labels": [], "assignees": []}]

    def request(self, method, path, body=None):
        self.calls.append((method, path, body))
        if method == "POST":
            if not self.token:
                raise ga.ApiError(403, "needs a token")
            return None
        if "/pulls" in path:
            return self.pulls
        if "/issues" in path:
            return self.issues
        if "/releases" in path:
            return [{"id": 9, "tag_name": "v1.1.2", "name": "1.1.2", "draft": False, "prerelease": False,
                     "published_at": iso(60), "html_url": "https://r", "assets": [{"download_count": 5}, {"download_count": 2}]},
                    {"id": 8, "tag_name": "v1.2.0-beta.1", "name": "", "draft": False, "prerelease": True,
                     "published_at": iso(30), "html_url": "https://r8", "assets": []}]
        if path == "/user":
            return {"login": "dev"}
        if "/check-runs" in path:
            return {"check_runs": [
                {"id": 501, "name": "tests", "status": "completed", "conclusion": "success", "app": {"slug": "github-actions"},
                 "started_at": iso(5), "completed_at": iso(3)},
                {"id": 502, "name": "codecov", "status": "in_progress", "conclusion": None, "app": {"slug": "codecov"},
                 "details_url": "https://codecov", "started_at": iso(2), "completed_at": None}]}
        if "/jobs" in path:
            return {"jobs": [{"id": 77, "name": "build", "status": "completed", "conclusion": "failure",
                              "started_at": iso(30), "completed_at": iso(28), "html_url": "https://x",
                              "steps": [{"number": 1, "name": "Checkout", "status": "completed",
                                         "conclusion": "success", "started_at": iso(30), "completed_at": iso(30)},
                                        {"number": 2, "name": "Test", "status": "completed",
                                         "conclusion": "failure", "started_at": iso(30), "completed_at": iso(28)}]}]}
        if "branch=" in path:
            branch = path.split("branch=")[1]
            return {"workflow_runs": [r for r in self.runs if r["head_branch"] == branch]}
        return {"workflow_runs": self.runs}


def capture(fn, *args):
    out = io.StringIO()
    with redirect_stdout(out):
        result = fn(*args)
    return result, [json.loads(line) for line in out.getvalue().splitlines()]


class RemoteTests(unittest.TestCase):
    def test_parse_remote(self):
        for url in ("git@github.com:DevStarByte/ByteForge.git", "https://github.com/DevStarByte/ByteForge",
                    "https://token@github.com/DevStarByte/ByteForge.git", "ssh://git@github.com/DevStarByte/ByteForge.git",
                    "https://github.com/DevStarByte/ByteForge/"):
            self.assertEqual(ga.parse_remote(url), ("DevStarByte", "ByteForge"), url)
        self.assertEqual(ga.parse_remote("git@github.com:me/my.repo.git"), ("me", "my.repo"))
        for url in ("git@gitlab.com:a/b.git", "https://github.com/onlyowner", "", None):
            self.assertIsNone(ga.parse_remote(url))

    def test_find_repository_prefers_upstream_remote(self):
        with tempfile.TemporaryDirectory() as root:
            def g(*args):
                subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)
            g("init", "-q", "-b", "main")
            g("remote", "add", "origin", "https://github.com/fork/repo.git")
            g("remote", "add", "upstream", "git@github.com:org/repo.git")
            self.assertEqual(ga.find_repository(root), ("fork", "repo", "main"))
            g("config", "branch.main.remote", "upstream")
            self.assertEqual(ga.find_repository(root), ("org", "repo", "main"))

    def test_dispatchable_workflows(self):
        with tempfile.TemporaryDirectory() as root:
            folder = os.path.join(root, ".github", "workflows")
            os.makedirs(folder)
            files = {"a.yml": "on:\n  push:\n  workflow_dispatch:\n", "b.yml": "on: [push, workflow_dispatch]\n",
                     "c.yaml": "on: workflow_dispatch\n", "d.yml": "on:\n  push:\n    branches: [main]\n"}
            for name, text in files.items():
                with open(os.path.join(folder, name), "w") as f:
                    f.write(text)
            self.assertEqual(ga.dispatchable_workflows(root), {"a.yml", "b.yml", "c.yaml"})


class PresentationTests(unittest.TestCase):
    def test_states(self):
        self.assertEqual(ga.state({"status": "in_progress"}), "running")
        self.assertEqual(ga.state({"status": "queued"}), "queued")
        self.assertEqual(ga.state({"status": "completed", "conclusion": "timed_out"}), "failure")
        self.assertEqual(ga.state({"status": "completed", "conclusion": "cancelled"}), "cancelled")
        self.assertEqual(ga.state({"status": "completed", "conclusion": "success"}), "success")

    def test_times(self):
        self.assertEqual(ga.ago(iso(90)), "1h ago")
        self.assertEqual(ga.ago(iso(0)), "just now")
        self.assertEqual(ga.duration("2026-01-01T10:00:00Z", "2026-01-01T10:02:05Z"), "2m 5s")
        self.assertEqual(ga.duration("2026-01-01T10:00:00Z", "2026-01-01T11:30:00Z"), "1h 30m")

    def test_clean_log_strips_timestamps_and_finds_the_first_error(self):
        raw = ("﻿2026-11-20T09:00:00.1234567Z ##[group]Run dotnet test\r\n"
               "2026-11-20T09:00:01.0000000Z Passed!\r\n"
               "2026-11-20T09:00:02.0000000Z ##[error]Process completed with exit code 1.\r\n")
        text, line = ga.clean_log(raw)
        self.assertEqual(text.split("\n")[:3], ["##[group]Run dotnet test", "Passed!", "##[error]Process completed with exit code 1."])
        self.assertEqual(line, 2)


class ModuleTests(unittest.TestCase):
    def module(self, token=None):
        a = ga.Actions()
        a.root = "/x"
        a.repo = ("o", "r", "master")
        a.api = FakeApi(token)
        return a

    def test_tree_groups_branch_runs_and_workflows(self):
        a = self.module()
        a.fetch()
        tree = a.tree()
        self.assertEqual([i["label"] for i in tree["items"]], ["Branch master", "Pull Requests", "Workflows", "Issues", "Releases"])
        branch, _, workflows, _, _ = tree["items"]
        self.assertEqual(branch["label"], "Branch master")
        self.assertEqual([c["label"] for c in branch["children"]], ["CI #3", "CI #2"])
        self.assertEqual(branch["children"][0]["iconColor"], ga.YELLOW)
        self.assertEqual([c["label"] for c in workflows["children"]], ["CI", "Release"])
        self.assertEqual(tree["badge"], 1)
        failed = branch["children"][1]
        self.assertIn("rerunFailed", [m["id"] for m in failed["menu"]])
        self.assertIn("cancel", [m["id"] for m in branch["children"][0]["menu"]])

    def test_click_on_a_run_lists_its_jobs_and_steps(self):
        a = self.module()
        a.fetch()
        _, messages = capture(a.run_action, 2, "click")
        tree = messages[-1]
        run_row = tree["items"][0]["children"][1]
        self.assertTrue(run_row["expanded"])
        job = run_row["children"][0]
        self.assertEqual(job["label"], "build")
        self.assertTrue(job["expanded"])  # failed jobs open on their steps
        self.assertEqual([s["label"] for s in job["children"]], ["Checkout", "Test"])
        self.assertEqual(job["children"][1]["iconColor"], ga.RED)
        capture(a.run_action, 2, "click")
        self.assertNotIn(2, a.open_runs)

    def test_rerun_without_token_explains_and_with_token_posts(self):
        a = self.module()
        a.fetch()
        _, messages = capture(a.run_action, 2, "rerunFailed")
        self.assertTrue(any("needs a GitHub token" in m.get("text", "") for m in messages))
        self.assertFalse([c for c in a.api.calls if c[0] == "POST"])
        a = self.module(token="t")
        a.fetch()
        ga.time.sleep, sleep = (lambda s: None), ga.time.sleep
        try:
            capture(a.run_action, 2, "rerunFailed")
        finally:
            ga.time.sleep = sleep
        self.assertIn(("POST", "/repos/o/r/actions/runs/2/rerun-failed-jobs", {}), a.api.calls)

    def test_status_bar_shows_the_latest_branch_run(self):
        a = self.module()
        a.fetch()
        run = lambda messages: next(m for m in messages if m["id"] == "github.run")
        _, messages = capture(a.update_status)
        self.assertEqual(run(messages)["text"], "● CI")
        a.settings = {"github.showStatus": False}
        _, messages = capture(a.update_status)
        self.assertEqual(run(messages)["text"], "")

    def test_not_a_github_folder(self):
        a = ga.Actions()
        a.root = "/x"
        self.assertIn("isn't a GitHub repository", a.tree()["message"])


class GitHubTests(unittest.TestCase):
    def module(self, token=None, branch="feature/x"):
        a = ga.Actions()
        a.root = "/x"
        a.repo = ("o", "r", branch)
        a.api = FakeApi(token)
        return a

    def test_branch_pull_request_comes_first_with_its_checks(self):
        a = self.module()
        a.fetch()
        pulls = a.tree()["items"][1]
        self.assertEqual(pulls["label"], "Pull Requests")
        first = pulls["children"][0]
        self.assertEqual(first["label"], "#12 Add export")
        self.assertTrue(first["description"].startswith("this branch"))
        self.assertEqual(first["iconColor"], ga.YELLOW)  # one check still running
        self.assertEqual([c["label"] for c in first["children"]], ["tests", "codecov"])
        self.assertIn("draft", pulls["children"][1]["description"])
        self.assertNotIn("children", pulls["children"][1])  # checks are asked for on a click

    def test_status_bar_shows_the_pull_request(self):
        a = self.module()
        a.fetch()
        _, messages = capture(a.update_status)
        pr = next(m for m in messages if m["id"] == "github.pr")
        self.assertEqual(pr["text"], "PR #12 ●")

    def test_click_on_a_pull_request_loads_its_checks(self):
        a = self.module(branch="master")
        a.fetch()
        _, messages = capture(a.pull_action, 7, "click")
        row = next(c for c in messages[-1]["items"][1]["children"] if c["id"] == "pr:7")
        self.assertTrue(row["expanded"])
        self.assertEqual(len(row["children"]), 2)

    def test_checkout_commands(self):
        a = self.module()
        a.fetch()
        self.assertEqual(ga.checkout_command(a.find_pull(12), "o/r"), "git fetch origin feature/x && git switch feature/x")
        self.assertEqual(ga.checkout_command(a.find_pull(7), "o/r"), "git fetch origin pull/7/head:pr-7 && git switch pr-7")

    def test_issues_leave_out_pull_requests_and_put_yours_first(self):
        a = self.module(token="t")
        a.fetch()
        issues = next(i for i in a.tree()["items"] if i["id"] == "issues")
        self.assertEqual([c["label"] for c in issues["children"]], ["#31 Dark mode", "#30 Crash on start!"])
        self.assertTrue(issues["children"][0]["description"].startswith("assigned to you"))

    def test_create_branch_from_issue(self):
        a = self.module()
        a.fetch()
        _, messages = capture(a.issue_action, 30, "branch")
        self.assertEqual(messages[0]["command"], "git switch -c feature/30-crash-on-start")
        a.settings = {"github.branchName": "{number}_{title}"}
        _, messages = capture(a.issue_action, 31, "branch")
        self.assertEqual(messages[0]["command"], "git switch -c 31_dark-mode")

    def test_releases(self):
        a = self.module()
        a.fetch()
        releases = next(i for i in a.tree()["items"] if i["id"] == "releases")
        stable, beta = sorted(releases["children"], key=lambda r: r["label"])
        self.assertEqual(stable["label"], "v1.1.2 1.1.2")
        self.assertTrue(stable["description"].startswith("latest"))
        self.assertIn("7 downloads", stable["description"])
        self.assertTrue(beta["description"].startswith("pre-release"))

    def test_pipelines_only_refresh_skips_the_rest(self):
        a = self.module()
        a.fetch(full=False)
        self.assertEqual(a.pulls, [])
        self.assertFalse([c for c in a.api.calls if "/pulls" in c[1]])


class ApiTests(unittest.TestCase):
    def test_token_from_setting_then_environment(self):
        self.assertEqual(ga.find_token({"github.token": " abc "}), ("abc", "the github.token setting"))
        old = os.environ.get("GH_TOKEN")
        os.environ["GH_TOKEN"] = "env"
        try:
            self.assertEqual(ga.find_token({}), ("env", "GH_TOKEN"))
        finally:
            if old is None:
                del os.environ["GH_TOKEN"]
            else:
                os.environ["GH_TOKEN"] = old

    def test_rate_limit_message(self):
        api = ga.Api()
        text = api.explain(403, b'{"message": "API rate limit exceeded"}',
                           {"X-RateLimit-Remaining": "0", "X-RateLimit-Reset": "0"})
        self.assertIn("rate limit", text)
        self.assertIn("github.token", text)


class ManifestTests(unittest.TestCase):
    def test_settings_and_view(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        self.assertEqual(manifest["views"][0]["id"], ga.VIEW)
        self.assertEqual({s["key"].split(".")[0] for s in manifest["settings"]}, {"github"})


if __name__ == "__main__":
    unittest.main()
