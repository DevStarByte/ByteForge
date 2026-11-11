//! Lists a package's versions from nuget.org. Needs the network, so it only runs on request:
//! `cargo test -p bf-core --test nuget_versions -- --ignored`.

#[test]
#[ignore]
fn lists_versions_latest_stable_first() {
    let dir = std::env::temp_dir();
    let versions = bf_core::nuget::versions(&dir, "Humanizer.Core").expect("dotnet package search");
    println!("{} versions, first: {:?}", versions.len(), &versions[..versions.len().min(4)]);
    assert!(versions.len() > 10);
    assert!(!bf_core::nuget::is_prerelease(&versions[0]), "{:?}", &versions[..3]);
    assert!(versions.iter().any(|v| bf_core::nuget::is_prerelease(v)), "prereleases are listed too");
    // Newest first after the latest stable.
    assert!(bf_core::nuget::compare_versions(&versions[1], &versions[versions.len() - 1]).is_gt());
}
