//! Puts netcoredbg into a running container and finds the app's process there. Needs a running .NET
//! container: set BF_TEST_CONTAINER to its name (skips otherwise), e.g. one built from Visual Studio's
//! Dockerfile template with `ENTRYPOINT ["dotnet", "App.dll"]`.

use bf_core::container;

#[test]
fn prepares_a_container_for_debugging() {
    let Ok(name) = std::env::var("BF_TEST_CONTAINER") else {
        println!("SKIPPED: set BF_TEST_CONTAINER to a running .NET container");
        return;
    };
    let containers = container::containers().expect("docker ps");
    assert!(containers.iter().any(|c| c.name == name), "{name} isn't running: {containers:?}");

    let apps = container::app_candidates(&container::processes(&name));
    println!("app processes: {apps:?}");
    assert!(apps.first().is_some_and(|p| p.command.contains("dotnet")), "{apps:?}");

    let arch = container::architecture(&name).expect("image architecture");
    let on_path = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join("netcoredbg")).find(|p| p.is_file()));
    let host = on_path.and_then(|exe| container::host_debugger_dir(&exe.to_string_lossy(), &arch));
    let dir = match host {
        Some(dir) => dir,
        None => container::download_debugger(&arch, &std::env::temp_dir().join("bf-netcoredbg-cache")).expect("download netcoredbg"),
    };
    println!("netcoredbg from {}", dir.display());
    container::install_debugger(&name, &dir).expect("install netcoredbg");
    assert!(container::debugger_ready(&name));
}
