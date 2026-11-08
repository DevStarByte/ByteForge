//! Test discovery over a project tree: only test projects, nested projects excluded, bin/obj skipped.

use bf_core::testing::discover;

#[test]
fn discovers_tests_in_test_projects_only() {
    let root = std::env::temp_dir().join(format!("bf-test-discovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("App/App.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>");
    write("App/Program.cs", "namespace App; public class P { [Fact] public void NotATest() {} }");
    write("Tests/Tests.csproj", "<Project><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" /></ItemGroup></Project>");
    write("Tests/MathTests.cs", "namespace Tests;\npublic class MathTests\n{\n    [Fact]\n    public void Adds() {}\n}\n");
    write("Tests/obj/Generated.cs", "namespace Tests; public class G { [Fact] public void Hidden() {} }");
    write("Tests/Inner/Inner.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>");
    write("Tests/Inner/Other.cs", "namespace Inner; public class O { [Fact] public void Elsewhere() {} }");

    let projects = discover(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "Tests");
    let names: Vec<String> = projects[0].classes.iter().flat_map(|c| c.tests.iter().map(|t| t.full_name())).collect();
    assert_eq!(names, ["Tests.MathTests.Adds"]);
}
