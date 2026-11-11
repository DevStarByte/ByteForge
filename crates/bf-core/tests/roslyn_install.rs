//! Installs the Roslyn language server from Microsoft's feed the way ByteForge does (newest version,
//! Microsoft's signature checked, server folder unpacked). Downloads ~40 MB, so it only runs on request:
//! `cargo test -p bf-core --test roslyn_install -- --ignored` (BF_ROSLYN_CACHE keeps the install).

#[test]
#[ignore]
fn installs_the_newest_server() {
    let cache = std::env::var_os("BF_ROSLYN_CACHE").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("bf-roslyn-cache-{}", std::process::id())));
    let (base, version) = bf_core::roslyn::latest().expect("the feed answers");
    println!("newest: {version}");
    let dll = bf_core::roslyn::install(&cache, &base, &version).expect("install");
    println!("installed: {}", dll.display());
    assert!(dll.is_file());
    let installed = bf_core::roslyn::installed(&cache);
    assert_eq!(installed.first().map(|(v, _)| v.as_str()), Some(version.as_str()));
    assert!(bf_core::roslyn::runtime_available(), "this machine has .NET 10+");
}
