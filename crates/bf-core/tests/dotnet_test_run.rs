//! Runs a real xUnit project the way the Test Explorer does: discover from source, `dotnet test` with a
//! filter and the TRX logger, read the results, and the host-debug process id line. Skips (passes) with a
//! message when `dotnet` or the xunit template isn't available.

use std::path::PathBuf;
use std::process::Command;

use bf_core::testing::{Outcome, discover, filter, host_debug_pid, parse_trx};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dotnet(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> Option<String> {
    let out = Command::new("dotnet").args(args).current_dir(dir).envs(env.iter().copied()).env("DOTNET_NOLOGO", "1").output().ok()?;
    Some(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn runs_filtered_tests_and_reads_trx() {
    let root = TempDir(std::env::temp_dir().join(format!("bf-dotnet-test-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root.0);
    std::fs::create_dir_all(&root.0).unwrap();
    if dotnet(&root.0, &["new", "xunit", "-o", "Calc.Tests", "-n", "Calc.Tests"], &[]).is_none() || !root.0.join("Calc.Tests/Calc.Tests.csproj").exists() {
        eprintln!("skipped: dotnet or the xunit template is not available");
        return;
    }
    let project_dir = root.0.join("Calc.Tests");
    std::fs::write(
        project_dir.join("UnitTest1.cs"),
        "namespace Calc.Tests;\n\npublic class MathTests\n{\n    [Fact]\n    public void Adds() => Assert.Equal(4, 2 + 2);\n\n    [Fact]\n    public void Fails() => Assert.Equal(5, 2 + 2);\n\n    [Theory]\n    [InlineData(1)]\n    [InlineData(2)]\n    public void Positive(int x) => Assert.True(x > 0);\n}\n",
    )
    .unwrap();

    let projects = discover(&root.0);
    assert_eq!(projects.len(), 1);
    let names: Vec<String> = projects[0].classes[0].tests.iter().map(|t| t.full_name()).collect();
    assert_eq!(names, ["Calc.Tests.MathTests.Adds", "Calc.Tests.MathTests.Fails", "Calc.Tests.MathTests.Positive"]);

    // The whole class, with TRX results.
    let results_dir = root.0.join("results");
    let class_filter = filter("Calc.Tests.MathTests", true);
    let project = projects[0].path.to_string_lossy().into_owned();
    let out = dotnet(&root.0, &["test", &project, "--logger", "trx", "--results-directory", &results_dir.to_string_lossy(), "--filter", &class_filter], &[]).unwrap();
    let trx = std::fs::read_dir(&results_dir).ok().and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "trx")));
    let Some(trx) = trx else {
        eprintln!("skipped: no TRX written (offline restore?):\n{out}");
        return;
    };
    let results = parse_trx(&std::fs::read_to_string(trx).unwrap()).unwrap();
    assert_eq!(results.len(), 4, "{results:?}");
    let outcome = |name: &str| results.iter().filter(|r| r.full_name == name).map(|r| r.outcome).collect::<Vec<_>>();
    assert_eq!(outcome("Calc.Tests.MathTests.Adds"), [Outcome::Passed]);
    assert_eq!(outcome("Calc.Tests.MathTests.Fails"), [Outcome::Failed]);
    assert_eq!(outcome("Calc.Tests.MathTests.Positive"), [Outcome::Passed, Outcome::Passed]);
    assert!(results.iter().find(|r| r.outcome == Outcome::Failed).and_then(|r| r.message.clone()).is_some_and(|m| m.contains("Assert.Equal")));

    // Live progress: the console logger reports each test as it finishes.
    let out = dotnet(&root.0, &["test", &project, "--no-build", "--logger", "console;verbosity=normal", "--filter", &class_filter], &[]).unwrap();
    let live: Vec<(Outcome, String)> = out.lines().filter_map(bf_core::testing::parse_progress_line).collect();
    assert_eq!(live.len(), 4, "{out}");
    assert!(live.iter().any(|(o, n)| *o == Outcome::Failed && n == "Calc.Tests.MathTests.Fails"), "{live:?}");
    assert_eq!(live.iter().filter(|(_, n)| n.starts_with("Calc.Tests.MathTests.Positive(")).count(), 2, "{live:?}");

    // One test of the class.
    let out = dotnet(&root.0, &["test", &project, "--no-build", "--filter", &filter("Calc.Tests.MathTests.Positive", false)], &[]).unwrap();
    assert!(out.contains("Passed:     2") && out.contains("Total:     2"), "{out}");

    // Host debugging prints the process id to attach to (the host then waits; we don't attach).
    let mut child = Command::new("dotnet")
        .args(["test", &project, "--no-build", "--filter", &filter("Calc.Tests.MathTests.Adds", false)])
        .current_dir(&root.0)
        .env("VSTEST_HOST_DEBUG", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let pid = std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok).find_map(|l| host_debug_pid(&l));
    let _ = child.kill();
    if let Some(pid) = pid {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    let _ = child.wait();
    assert!(pid.is_some(), "no 'Process Id:' line from VSTEST_HOST_DEBUG");
}
