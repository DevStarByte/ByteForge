//! The Windows Forms designer in a tab: opening it for a form (View Designer, Shift+F7, or the tab strip's
//! button on a form's code), View Code (F7), and the event handlers it adds to the form's code.

use bf_core::winforms;

use super::*;
use crate::form_designer::{DesignerEvent, FormDesigner};

impl Workbench {
    /// Whether a file is a form or user control the designer can open: `Form1.cs` next to a
    /// `Form1.Designer.cs` with InitializeComponent (looked up once per path).
    pub(super) fn is_designable(&mut self, path: &Path) -> bool {
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cs")) {
            return false;
        }
        if let Some(&known) = self.designable.get(path) {
            return known;
        }
        let yes = winforms::designer_pair(path).is_some();
        self.designable.insert(path.to_path_buf(), yes);
        yes
    }

    /// "View Designer": the designer of the current file's form.
    pub(super) fn open_form_designer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else {
            self.log("View Designer: open a form's .cs file first.");
            return;
        };
        if matches!(self.tabs[ix].page, Some(Page::Designer(_))) {
            return;
        }
        let Some(path) = self.tabs[ix].path.clone().filter(|_| self.tabs[ix].is_document()) else {
            self.log("View Designer: open a form's .cs file first.");
            return;
        };
        self.open_designer_for(path, window, cx);
    }

    /// Opens (or shows) the designer of the form `path` belongs to (its code or its .Designer.cs).
    pub(crate) fn open_designer_for(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some((code, designer)) = winforms::designer_pair(&path) else {
            self.log(format!("View Designer: {name} isn't a form or user control (there is no .Designer.cs with InitializeComponent next to it)."));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.designable.insert(code.clone(), true);
        if let Some(i) = self.tabs.iter().position(|t| matches!(&t.page, Some(Page::Designer(d)) if bf_core::paths::same_path(&d.read(cx).designer_path, &designer))) {
            self.activate(i, window, cx);
            return;
        }
        let bytes = match std::fs::read(&designer) {
            Ok(b) => b,
            Err(e) => {
                self.log(format!("Could not open {}: {e}", designer.display()));
                return;
            }
        };
        let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let saved = String::from_utf8_lossy(if bom { &bytes[3..] } else { &bytes[..] }).into_owned();
        let editor = cx.new(|cx| EditorView::new(&saved, Some(designer.clone()), cx));
        // The file open as code with unsaved edits: the designer starts from those.
        let open_text = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &designer))).map(|t| t.editor.read(cx).state.buffer.text());
        editor.update(cx, |e, cx| {
            self.configure_editor(e);
            if let Some(text) = &open_text {
                e.replace_text_minimal(text, cx);
                if *text == saved.replace("\r\n", "\n") {
                    e.state.buffer.mark_saved();
                }
            }
        });
        // The base class (Form, UserControl) is in the code file.
        let class = winforms::Designer::parse(&editor.read(cx).state.buffer.text(), "Form").map(|d| d.class_name).unwrap_or_default();
        let root_type = std::fs::read_to_string(&code).ok().and_then(|c| winforms::base_type(&c, &class)).unwrap_or_else(|| "Form".into());
        let view = cx.new(|cx| FormDesigner::new(editor.clone(), code.clone(), designer.clone(), root_type, cx));
        let sub = cx.subscribe_in(&view, window, Self::on_designer_event);
        let title = format!("{} [Design]", code.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        self.add_tab(editor, Some(designer), title, bom, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Designer(view.clone()));
            tab._subs.push(sub);
        }
        view.read(cx).focus(window);
        cx.notify();
    }

    /// "View Code" (F7): from a designer, its form's code.
    pub(super) fn view_form_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.active.and_then(|ix| match &self.tabs[ix].page {
            Some(Page::Designer(d)) => Some(d.read(cx).code_path.clone()),
            _ => None,
        });
        match code {
            Some(code) => self.open_file(code, None, window, cx),
            None => self.log("View Code: open a form's designer first."),
        }
    }

    fn on_designer_event(&mut self, view: &Entity<FormDesigner>, event: &DesignerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let code = view.read(cx).code_path.clone();
        match event {
            DesignerEvent::ViewCode => self.open_file(code, None, window, cx),
            DesignerEvent::Handler { method, args } => {
                let Some(class) = view.read(cx).class_name() else { return };
                self.open_file(code.clone(), None, window, cx);
                let Some(editor) = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &code))).map(|t| t.editor.clone()) else { return };
                let text = editor.read(cx).state.buffer.text();
                match winforms::add_handler(&text, &class, method, args) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    const CODE: &str = "namespace App;\n\npublic partial class Form1 : Form\n{\n    public Form1()\n    {\n        InitializeComponent();\n    }\n}\n";
    const DESIGNER: &str = "namespace App;\n\npartial class Form1\n{\n    private System.ComponentModel.IContainer components = null;\n\n    private void InitializeComponent()\n    {\n        button1 = new Button();\n        SuspendLayout();\n        // \n        // button1\n        // \n        button1.Location = new Point(12, 12);\n        button1.Name = \"button1\";\n        button1.Size = new Size(75, 23);\n        button1.TabIndex = 0;\n        button1.Text = \"OK\";\n        button1.UseVisualStyleBackColor = true;\n        // \n        // Form1\n        // \n        AutoScaleDimensions = new SizeF(7F, 15F);\n        AutoScaleMode = AutoScaleMode.Font;\n        ClientSize = new Size(400, 300);\n        Controls.Add(button1);\n        Name = \"Form1\";\n        Text = \"Form1\";\n        ResumeLayout(false);\n    }\n\n    private Button button1;\n}\n";

    fn selector(name: String) -> &'static str {
        Box::leak(name.into_boxed_str())
    }

    fn bounds(cx: &mut gpui::VisualTestContext, name: &str) -> gpui::Bounds<gpui::Pixels> {
        cx.run_until_parked();
        cx.debug_bounds(selector(name.to_owned())).unwrap_or_else(|| panic!("{name} is drawn"))
    }

    fn click(cx: &mut gpui::VisualTestContext, name: &str) {
        let b = bounds(cx, name);
        cx.simulate_click(b.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    fn designer_of(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Entity<FormDesigner> {
        wb.read_with(cx, |wb, _| match wb.active.and_then(|i| wb.tabs[i].page.clone()) {
            Some(Page::Designer(d)) => d,
            _ => panic!("the active tab is a designer"),
        })
    }

    /// Shift+F7 on a form's code opens its designer; a control placed from the toolbox, a property typed in
    /// the grid and a double click that adds a Click handler all end up in the files.
    #[gpui::test]
    fn designs_a_form_with_the_mouse(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "designer");
        let dir = guard.dir.join("ws");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Form1.cs"), CODE).unwrap();
        std::fs::write(dir.join("Form1.Designer.cs"), DESIGNER).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(dir.clone(), window, cx);
            wb.open_file(dir.join("Form1.cs"), None, window, cx);
        });
        cx.run_until_parked();
        // The tab strip offers the designer on a form's code.
        bounds(cx, "designer-button-0");
        cx.dispatch_action(ViewDesigner);
        cx.run_until_parked();
        let designer = designer_of(&wb, cx);
        wb.read_with(cx, |wb, _| assert_eq!(wb.tabs[wb.active.unwrap()].title, "Form1.cs [Design]"));

        // Pick the TextBox in the toolbox, click on the form: a textBox1 at that point.
        click(cx, "designer-tool-TextBox");
        let form = bounds(cx, "designer-form-client");
        cx.simulate_click(form.origin + gpui::point(px(100.), px(80.)), gpui::Modifiers::none());
        cx.run_until_parked();
        let text = || std::fs::read_to_string(dir.join("Form1.Designer.cs")).unwrap();
        designer.read_with(cx, |d, _| {
            assert_eq!(d.selection(), ["textBox1"]);
            let m = d.model().unwrap();
            assert_eq!(m.location("textBox1"), (100, 80));
            assert_eq!(m.parent("textBox1"), Some(("Form1", "")));
        });
        assert_eq!(text(), DESIGNER, "not saved yet");

        // Type its Text in the grid.
        click(cx, "designer-prop-Text");
        cx.simulate_input("Hello");
        cx.press("enter");
        cx.run_until_parked();
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().text_of("textBox1").as_deref(), Some("Hello")));

        // Drag the button 20 pixels right.
        let b = form.origin + gpui::point(px(30.), px(20.));
        cx.simulate_mouse_down(b, MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_move(b + gpui::point(px(10.), px(0.)), Some(MouseButton::Left), gpui::Modifiers::none());
        cx.simulate_mouse_move(b + gpui::point(px(20.), px(0.)), Some(MouseButton::Left), gpui::Modifiers::none());
        cx.simulate_mouse_up(b + gpui::point(px(20.), px(0.)), MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().location("button1"), (32, 12)));
        // Ctrl+Z / Ctrl+Y go through the file's history.
        cx.press("ctrl-z");
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().location("button1"), (12, 12)));
        cx.press("ctrl-y");
        designer.read_with(cx, |d, _| assert_eq!(d.model().unwrap().location("button1"), (32, 12)));

        // Ctrl+S saves the .Designer.cs; it reads back to the same form.
        cx.dispatch_action(Save);
        cx.run_until_parked();
        let saved = text();
        assert!(saved.contains("        textBox1 = new TextBox();\n        button1 = new Button();") || saved.contains("        button1 = new Button();\n        textBox1 = new TextBox();"), "{saved}");
        assert!(saved.contains("        textBox1.Text = \"Hello\";"), "{saved}");
        assert!(saved.contains("        button1.Location = new Point(32, 12);"), "{saved}");
        assert!(saved.contains("        Controls.Add(textBox1);\n        Controls.Add(button1);"), "{saved}");
        assert!(saved.contains("    private Button button1;\n    private TextBox textBox1;\n}"), "{saved}");

        // Double click on the button: button1_Click in Form1.cs, hooked up in the designer file.
        let b = form.origin + gpui::point(px(40.), px(20.));
        cx.simulate_event(gpui::MouseDownEvent { position: b, button: MouseButton::Left, modifiers: gpui::Modifiers::none(), click_count: 2, first_mouse: false });
        cx.simulate_event(gpui::MouseUpEvent { position: b, button: MouseButton::Left, modifiers: gpui::Modifiers::none(), click_count: 2 });
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            let tab = &wb.tabs[wb.active.unwrap()];
            assert_eq!(tab.title, "Form1.cs", "the code opens");
            let code = tab.editor.read(cx).state.buffer.text();
            assert!(code.contains("    private void button1_Click(object sender, EventArgs e)\n    {\n        \n    }\n}"), "{code}");
            assert_eq!(tab.editor.read(cx).state.caret(), Point { line: 11, col: 8 });
            let designer_tab = wb.tabs.iter().find(|t| matches!(t.page, Some(Page::Designer(_)))).unwrap();
            assert!(designer_tab.editor.read(cx).state.buffer.text().contains("        button1.Click += button1_Click;"));
        });
    }
}
