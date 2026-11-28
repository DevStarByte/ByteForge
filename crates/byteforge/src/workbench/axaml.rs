//! The Avalonia designer in a tab: opening it for an `.axaml` view (View Designer, Shift+F7, or the tab
//! strip's button), View Code (F7), the code's caret selecting what it is on, and the event handlers it adds
//! to the view's code-behind.

use bf_core::winforms;

use super::*;
use crate::axaml_designer::{AxamlDesigner, AxamlEvent, code_behind};

impl Workbench {
    /// Opens (or shows) the designer of an `.axaml` view.
    pub(super) fn open_axaml_designer(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(&t.page, Some(Page::Axaml(d)) if bf_core::paths::same_path(&d.read(cx).path, &path))) {
            self.activate(i, window, cx);
            return;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.log(format!("Could not open {}: {e}", path.display()));
                return;
            }
        };
        let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let saved = String::from_utf8_lossy(if bom { &bytes[3..] } else { &bytes[..] }).into_owned();
        let editor = cx.new(|cx| EditorView::new(&saved, Some(path.clone()), cx));
        // The view open as code with unsaved edits: the designer starts from those (and stays in step with
        // that tab: edits in either show in the other).
        let open_text = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))).map(|t| t.editor.read(cx).state.buffer.text());
        editor.update(cx, |e, cx| {
            self.configure_editor(e);
            if let Some(text) = &open_text {
                e.replace_text_minimal(text, cx);
                if *text == saved.replace("\r\n", "\n") {
                    e.state.buffer.mark_saved();
                }
            }
        });
        let view = cx.new(|cx| AxamlDesigner::new(editor.clone(), path.clone(), cx));
        let sub = cx.subscribe_in(&view, window, Self::on_axaml_event);
        let title = format!("{} [Design]", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        self.add_tab(editor, Some(path), title, bom, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Axaml(view.clone()));
            tab._subs.push(sub);
        }
        view.read(cx).focus(window);
        cx.notify();
    }

    /// The code's caret moved in an `.axaml` file: its open designer selects the element there.
    pub(super) fn follow_axaml_caret(&mut self, editor: &Entity<EditorView>, window: &Window, cx: &mut Context<Self>) {
        // Only the caret moved by hand (not by an edit the designer made and the code tab mirrors).
        if !editor.focus_handle(cx).is_focused(window) {
            return;
        }
        let Some(path) = self.tabs.iter().find(|t| t.editor == *editor && t.is_document()).and_then(|t| t.path.clone()) else { return };
        if !crate::axaml_designer::is_axaml(&path) {
            return;
        }
        let line = editor.read(cx).state.caret().line;
        for t in &self.tabs {
            if let Some(Page::Axaml(d)) = &t.page
                && bf_core::paths::same_path(&d.read(cx).path, &path)
            {
                d.update(cx, |d, cx| d.select_line(line, cx));
            }
        }
    }

    fn on_axaml_event(&mut self, view: &Entity<AxamlDesigner>, event: &AxamlEvent, window: &mut Window, cx: &mut Context<Self>) {
        let path = view.read(cx).path.clone();
        match event {
            AxamlEvent::ViewCode { line } => self.open_file(path, line.map(|line| Point { line, col: 0 }), window, cx),
            AxamlEvent::Handler { method, args, namespace } => {
                let Some(class) = view.read(cx).class_name() else { return };
                let code = code_behind(&path);
                if !code.is_file() {
                    let file = code.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.log(format!("Could not add {method}: there is no {file} next to the view."));
                    self.show_panel(PanelView::Output, cx);
                    return;
                }
                self.open_file(code.clone(), None, window, cx);
                let Some(editor) = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &code))).map(|t| t.editor.clone()) else { return };
                let text = editor.read(cx).state.buffer.text();
                let with_using = add_using(&text, namespace);
                match winforms::add_handler(&with_using, &class, method, args) {
                    Some((new, line, col)) => editor.update(cx, |e, cx| {
                        if new != text {
                            e.replace_text_minimal(&new, cx);
                        }
                        e.go_to(Point { line, col }, cx);
                        e.focus(window);
                    }),
                    None => {
                        let file = code.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.log(format!("Could not add {method}: {file} has no class {class}."));
                        self.show_panel(PanelView::Output, cx);
                    }
                }
            }
        }
    }
}

/// `using <namespace>;` added to C# code after its other usings (unless it is there, or is global).
fn add_using(code: &str, namespace: &str) -> String {
    // `System` comes with the implicit usings of every new project.
    let line = format!("using {namespace};");
    if code.lines().any(|l| l.trim() == line) || namespace == "System" {
        return code.to_owned();
    }
    let mut out = String::with_capacity(code.len() + line.len() + 1);
    let last_using = code.lines().enumerate().filter(|(_, l)| l.trim_start().starts_with("using ") && l.trim_end().ends_with(';')).map(|(i, _)| i).last();
    match last_using {
        Some(i) => {
            for (j, l) in code.split_inclusive('\n').enumerate() {
                out.push_str(l);
                if j == i {
                    if !l.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&line);
                    out.push('\n');
                }
            }
        }
        None => {
            out.push_str(&line);
            out.push_str("\n\n");
            out.push_str(code);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    #[test]
    fn adds_usings_after_the_others() {
        let code = "using Avalonia.Controls;\nusing System;\n\nnamespace App;\n";
        assert_eq!(add_using(code, "Avalonia.Interactivity"), "using Avalonia.Controls;\nusing System;\nusing Avalonia.Interactivity;\n\nnamespace App;\n");
        assert_eq!(add_using(code, "Avalonia.Controls"), code);
        assert_eq!(add_using("namespace App;\n", "Avalonia.Input"), "using Avalonia.Input;\n\nnamespace App;\n");
    }

    const VIEW: &str = r#"<Window xmlns="https://github.com/avaloniaui"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        xmlns:d="http://schemas.microsoft.com/expression/blend/2008"
        xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
        mc:Ignorable="d" d:DesignWidth="400" d:DesignHeight="300"
        x:Class="App.MainWindow"
        Title="App">
    <StackPanel Margin="10" Spacing="4">
        <TextBlock Text="Hello"/>
        <Button Content="OK"/>
    </StackPanel>
</Window>
"#;
    const CODE: &str = "using Avalonia.Controls;\n\nnamespace App;\n\npublic partial class MainWindow : Window\n{\n    public MainWindow()\n    {\n        InitializeComponent();\n    }\n}\n";

    fn selector(name: &str) -> &'static str {
        Box::leak(name.to_owned().into_boxed_str())
    }

    fn bounds(cx: &mut gpui::VisualTestContext, name: &str) -> gpui::Bounds<gpui::Pixels> {
        cx.run_until_parked();
        cx.debug_bounds(selector(name)).unwrap_or_else(|| panic!("{name} is drawn"))
    }

    fn click(cx: &mut gpui::VisualTestContext, name: &str) {
        let b = bounds(cx, name);
        cx.simulate_click(b.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    fn designer_of(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Entity<AxamlDesigner> {
        wb.read_with(cx, |wb, _| match wb.active.and_then(|i| wb.tabs[i].page.clone()) {
            Some(Page::Axaml(d)) => d,
            _ => panic!("the active tab is the Avalonia designer"),
        })
    }

    /// Shift+F7 on a view opens its designer; a control from the toolbox dropped into the stack panel, a
    /// property typed in the grid, a drag that reorders, typing in the XAML and a double click that adds a
    /// handler to the code-behind all go through the file.
    #[gpui::test]
    fn designs_an_avalonia_view(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "axaml-designer");
        let dir = guard.dir.join("ws");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MainWindow.axaml"), VIEW).unwrap();
        std::fs::write(dir.join("MainWindow.axaml.cs"), CODE).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(dir.clone(), window, cx);
            wb.open_file(dir.join("MainWindow.axaml"), None, window, cx);
        });
        cx.run_until_parked();
        bounds(cx, "designer-button-0");
        cx.dispatch_action(ViewDesigner);
        cx.run_until_parked();
        let designer = designer_of(&wb, cx);
        wb.read_with(cx, |wb, _| assert_eq!(wb.tabs[wb.active.unwrap()].title, "MainWindow.axaml [Design]"));
        let text = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| {
            wb.read_with(cx, |wb, cx| wb.tabs.iter().find(|t| matches!(t.page, Some(Page::Axaml(_)))).unwrap().editor.read(cx).state.buffer.text())
        };

        // Pick the CheckBox and click below the button: it goes at the end of the stack panel.
        click(cx, "axaml-tool-CheckBox");
        let view = bounds(cx, "axaml-view");
        cx.simulate_click(view.origin + gpui::point(px(200.), px(150.)), gpui::Modifiers::none());
        cx.run_until_parked();
        designer.read_with(cx, |d, _| assert_eq!(d.selection(), Some(&vec![0, 2]), "{:?} {:?}", d.message(), d.layout_rect(&[0])));
        assert!(text(&wb, cx).contains("        <Button Content=\"OK\"/>\n        <CheckBox Content=\"CheckBox\"/>\n    </StackPanel>"), "{}", text(&wb, cx));
        // The code tab follows (it's the same file).
        wb.read_with(cx, |wb, cx| {
            let code_tab = wb.tabs.iter().find(|t| t.is_document()).unwrap();
            assert!(code_tab.editor.read(cx).state.buffer.text().contains("<CheckBox Content=\"CheckBox\"/>"));
        });

        // Type its content in the grid.
        click(cx, "axaml-prop-Content");
        cx.simulate_input("Remember me");
        cx.press("enter");
        cx.run_until_parked();
        assert!(text(&wb, cx).contains("<CheckBox Content=\"Remember me\"/>"), "{}", text(&wb, cx));

        // Drag it above the text block (Hello is at 10..29 in the view).
        let from = view.origin + gpui::point(px(30.), px(100.));
        let to = view.origin + gpui::point(px(100.), px(12.));
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_move(from + gpui::point(px(0.), px(-20.)), Some(MouseButton::Left), gpui::Modifiers::none());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        designer.read_with(cx, |d, _| {
            assert_eq!(d.selection(), Some(&vec![0, 0]));
            assert_eq!(d.model().unwrap().get(&[0, 0]).unwrap().tag, "CheckBox");
        });
        // Undone and redone through the file's history.
        cx.press("ctrl-z");
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().get(&[0, 0]).unwrap().tag, "TextBlock"));
        cx.press("ctrl-y");
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().get(&[0, 0]).unwrap().tag, "CheckBox"));

        // Typing a broken tag in the code keeps the last good view and says so; fixing it reads it again.
        let code_editor = wb.read_with(cx, |wb, _| wb.tabs.iter().find(|t| t.is_document()).unwrap().editor.clone());
        let good = text(&wb, cx);
        code_editor.update(cx, |e, cx| e.replace_text_minimal(&good.replace("<TextBlock Text=\"Hello\"/>", "<TextBlock Text=\"Hello\">"), cx));
        cx.run_until_parked();
        designer.read_with(cx, |d, _| {
            assert!(d.error().is_some());
            assert!(d.model().is_some());
        });
        code_editor.update(cx, |e, cx| e.replace_text_minimal(&good.replace("<TextBlock Text=\"Hello\"/>", "<TextBlock Text=\"Hi\"/>"), cx));
        cx.run_until_parked();
        designer.read_with(cx, |d, _| {
            assert!(d.error().is_none());
            assert_eq!(d.model().unwrap().get(&[0, 1]).unwrap().attr("Text"), Some("Hi"));
        });

        // Ctrl+S saves the view.
        cx.dispatch_action(Save);
        cx.run_until_parked();
        let saved = std::fs::read_to_string(dir.join("MainWindow.axaml")).unwrap();
        assert!(saved.contains("    <StackPanel Margin=\"10\" Spacing=\"4\">\n        <CheckBox Content=\"Remember me\"/>\n        <TextBlock Text=\"Hi\"/>"), "{saved}");

        // Double click on the button: Button_OnClick in the code-behind, hooked up in the view.
        let ok = designer.read_with(cx, |d, _| d.layout_rect(&[0, 2]).unwrap());
        let p = view.origin + gpui::point(px(ok.x + 5.), px(ok.y + 5.));
        cx.simulate_event(gpui::MouseDownEvent { position: p, button: MouseButton::Left, modifiers: gpui::Modifiers::none(), click_count: 2, first_mouse: false });
        cx.simulate_event(gpui::MouseUpEvent { position: p, button: MouseButton::Left, modifiers: gpui::Modifiers::none(), click_count: 2 });
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            let tab = &wb.tabs[wb.active.unwrap()];
            assert_eq!(tab.title, "MainWindow.axaml.cs", "the code-behind opens");
            let code = tab.editor.read(cx).state.buffer.text();
            assert!(code.contains("using Avalonia.Interactivity;"), "{code}");
            assert!(code.contains("    private void Button_OnClick(object sender, RoutedEventArgs e)\n    {\n        \n    }\n}"), "{code}");
        });
        assert!(text(&wb, cx).contains("<Button Content=\"OK\" Click=\"Button_OnClick\"/>"), "{}", text(&wb, cx));
    }
}
