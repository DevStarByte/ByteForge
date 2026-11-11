//! The Solution view's actions in the workbench: build, run and test a project, open and reveal files.

use super::*;
use crate::solution::SolutionEvent;
use crate::testing::TestRun;

impl Workbench {
    pub(super) fn on_solution_event(&mut self, _: &Entity<SolutionView>, event: &SolutionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SolutionEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            SolutionEvent::Build(path) => self.dotnet_on("build", path, window, cx),
            SolutionEvent::Run(path) => self.dotnet_on("run", path, window, cx),
            SolutionEvent::Watch(path) => self.dotnet_on("watch", path, window, cx),
            SolutionEvent::Test(path) => {
                let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.run_tests(vec![TestRun { project: path.clone(), filter: None, label }], false, window, cx);
            }
            SolutionEvent::AddPackage(project) => self.open_package_search(project.clone(), window, cx),
            SolutionEvent::UpdatePackage(project, id, version) => self.add_package(project.clone(), id.clone(), Some(version.clone()), window, cx),
            SolutionEvent::RemovePackage(project, id) => self.remove_package(project.clone(), id.clone(), window, cx),
            SolutionEvent::ChangePackageVersion(project, id, installed) => self.open_version_picker(project.clone(), id.clone(), Some(installed.clone()), window, cx),
            SolutionEvent::Reveal(path) => {
                self.show_side(SideView::Explorer, cx);
                self.explorer.update(cx, |e, cx| e.reveal(path, cx));
            }
        }
    }

    /// `dotnet build <target>`, `dotnet run --project <target>` or `dotnet watch` for one project or solution.
    fn dotnet_on(&mut self, verb: &str, target: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if self.dotnet_busy(cx) {
            return;
        }
        self.save_all(window, cx);
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet_verb(root, verb, target, window, cx);
    }
}
