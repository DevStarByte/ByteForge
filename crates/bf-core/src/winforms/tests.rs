use std::collections::HashMap;

use super::*;

/// What Visual Studio 2022 writes for a .NET 8 form.
pub(crate) const NET8: &str = r#"namespace WinFormsApp1
{
    partial class Form1
    {
        /// <summary>
        ///  Required designer variable.
        /// </summary>
        private System.ComponentModel.IContainer components = null;

        /// <summary>
        ///  Clean up any resources being used.
        /// </summary>
        /// <param name="disposing">true if managed resources should be disposed; otherwise, false.</param>
        protected override void Dispose(bool disposing)
        {
            if (disposing && (components != null))
            {
                components.Dispose();
            }
            base.Dispose(disposing);
        }

        #region Windows Form Designer generated code

        /// <summary>
        ///  Required method for Designer support - do not modify
        ///  the contents of this method with the code editor.
        /// </summary>
        private void InitializeComponent()
        {
            components = new System.ComponentModel.Container();
            button1 = new Button();
            groupBox1 = new GroupBox();
            checkBox1 = new CheckBox();
            comboBox1 = new ComboBox();
            timer1 = new System.Windows.Forms.Timer(components);
            menuStrip1 = new MenuStrip();
            fileToolStripMenuItem = new ToolStripMenuItem();
            groupBox1.SuspendLayout();
            menuStrip1.SuspendLayout();
            SuspendLayout();
            //
            // button1
            //
            button1.Anchor = AnchorStyles.Bottom | AnchorStyles.Right;
            button1.Location = new Point(713, 415);
            button1.Name = "button1";
            button1.Size = new Size(75, 23);
            button1.TabIndex = 0;
            button1.Text = "OK";
            button1.UseVisualStyleBackColor = true;
            button1.Click += button1_Click;
            //
            // groupBox1
            //
            groupBox1.Controls.Add(checkBox1);
            groupBox1.Location = new Point(12, 40);
            groupBox1.Name = "groupBox1";
            groupBox1.Size = new Size(200, 100);
            groupBox1.TabIndex = 1;
            groupBox1.TabStop = false;
            groupBox1.Text = "Options";
            //
            // checkBox1
            //
            checkBox1.AutoSize = true;
            checkBox1.Location = new Point(6, 22);
            checkBox1.Name = "checkBox1";
            checkBox1.Size = new Size(83, 19);
            checkBox1.TabIndex = 0;
            checkBox1.Text = "checkBox1";
            checkBox1.UseVisualStyleBackColor = true;
            //
            // comboBox1
            //
            comboBox1.FormattingEnabled = true;
            comboBox1.Items.AddRange(new object[] { "One", "Two" });
            comboBox1.Location = new Point(230, 40);
            comboBox1.Name = "comboBox1";
            comboBox1.Size = new Size(121, 23);
            comboBox1.TabIndex = 2;
            //
            // timer1
            //
            timer1.Interval = 500;
            timer1.Tick += timer1_Tick;
            //
            // menuStrip1
            //
            menuStrip1.Items.AddRange(new ToolStripItem[] { fileToolStripMenuItem });
            menuStrip1.Location = new Point(0, 0);
            menuStrip1.Name = "menuStrip1";
            menuStrip1.Size = new Size(800, 24);
            menuStrip1.TabIndex = 3;
            menuStrip1.Text = "menuStrip1";
            //
            // fileToolStripMenuItem
            //
            fileToolStripMenuItem.Name = "fileToolStripMenuItem";
            fileToolStripMenuItem.Size = new Size(37, 20);
            fileToolStripMenuItem.Text = "File";
            //
            // Form1
            //
            AutoScaleDimensions = new SizeF(7F, 15F);
            AutoScaleMode = AutoScaleMode.Font;
            ClientSize = new Size(800, 450);
            Controls.Add(comboBox1);
            Controls.Add(groupBox1);
            Controls.Add(button1);
            Controls.Add(menuStrip1);
            MainMenuStrip = menuStrip1;
            Name = "Form1";
            Text = "Main Window";
            Load += Form1_Load;
            groupBox1.ResumeLayout(false);
            groupBox1.PerformLayout();
            menuStrip1.ResumeLayout(false);
            menuStrip1.PerformLayout();
            ResumeLayout(false);
            PerformLayout();
        }

        #endregion

        private Button button1;
        private GroupBox groupBox1;
        private CheckBox checkBox1;
        private ComboBox comboBox1;
        private System.Windows.Forms.Timer timer1;
        private MenuStrip menuStrip1;
        private ToolStripMenuItem fileToolStripMenuItem;
    }
}
"#;

/// What Visual Studio writes for a .NET Framework form.
pub(crate) const FRAMEWORK: &str = r#"namespace LegacyApp
{
    partial class MainForm
    {
        private System.ComponentModel.IContainer components = null;

        #region Windows Form Designer generated code

        private void InitializeComponent()
        {
            this.label1 = new System.Windows.Forms.Label();
            this.pictureBox1 = new System.Windows.Forms.PictureBox();
            ((System.ComponentModel.ISupportInitialize)(this.pictureBox1)).BeginInit();
            this.SuspendLayout();
            //
            // label1
            //
            this.label1.AutoSize = true;
            this.label1.Font = new System.Drawing.Font("Microsoft Sans Serif", 12F, System.Drawing.FontStyle.Bold, System.Drawing.GraphicsUnit.Point, ((byte)(0)));
            this.label1.ForeColor = System.Drawing.Color.FromArgb(((int)(((byte)(192)))), ((int)(((byte)(0)))), ((int)(((byte)(0)))));
            this.label1.Location = new System.Drawing.Point(12, 9);
            this.label1.Name = "label1";
            this.label1.Size = new System.Drawing.Size(57, 20);
            this.label1.TabIndex = 0;
            this.label1.Text = "Hello";
            this.label1.Click += new System.EventHandler(this.label1_Click);
            //
            // pictureBox1
            //
            this.pictureBox1.Dock = System.Windows.Forms.DockStyle.Bottom;
            this.pictureBox1.Location = new System.Drawing.Point(0, 211);
            this.pictureBox1.Name = "pictureBox1";
            this.pictureBox1.Size = new System.Drawing.Size(284, 50);
            this.pictureBox1.TabIndex = 1;
            this.pictureBox1.TabStop = false;
            //
            // MainForm
            //
            this.AutoScaleDimensions = new System.Drawing.SizeF(6F, 13F);
            this.AutoScaleMode = System.Windows.Forms.AutoScaleMode.Font;
            this.ClientSize = new System.Drawing.Size(284, 261);
            this.Controls.Add(this.pictureBox1);
            this.Controls.Add(this.label1);
            this.Name = "MainForm";
            this.Text = "Legacy";
            ((System.ComponentModel.ISupportInitialize)(this.pictureBox1)).EndInit();
            this.ResumeLayout(false);
            this.PerformLayout();

        }

        #endregion

        private System.Windows.Forms.Label label1;
        private System.Windows.Forms.PictureBox pictureBox1;
    }
}
"#;

/// What `dotnet new winforms` creates (never opened in Visual Studio).
const TEMPLATE: &str = "namespace App;\n\npartial class Form1\n{\n    private System.ComponentModel.IContainer components = null;\n\n    #region Windows Form Designer generated code\n\n    private void InitializeComponent()\n    {\n        this.components = new System.ComponentModel.Container();\n        this.AutoScaleMode = System.Windows.Forms.AutoScaleMode.Font;\n        this.ClientSize = new System.Drawing.Size(800, 450);\n        this.Text = \"Form1\";\n    }\n\n    #endregion\n}\n";

/// A fixture as Visual Studio writes it: the empty comments around a block's name end in a space (which
/// editors like to strip from this file).
fn vs(text: &str) -> String {
    text.replace("//\n", "// \n")
}

fn net8() -> Designer {
    Designer::parse(&vs(NET8), "Form").unwrap()
}

fn framework() -> Designer {
    Designer::parse(&vs(FRAMEWORK), "Form").unwrap()
}

#[test]
fn reading_and_writing_back_changes_nothing() {
    assert_eq!(net8().to_text(), vs(NET8));
    assert_eq!(framework().to_text(), vs(FRAMEWORK));
    // Without the spaces, new blocks don't get them either.
    let mut bare = Designer::parse(NET8, "Form").unwrap();
    assert_eq!(bare.to_text(), NET8);
    bare.add_component("Label", None, (1, 1), None);
    assert!(bare.to_text().contains("            //\n            // label1\n            //\n"));
    // CRLF files are read as LF (the editor's buffer is LF).
    assert_eq!(Designer::parse(&vs(NET8).replace('\n', "\r\n"), "Form").unwrap().to_text(), vs(NET8));
}

#[test]
fn reads_the_model() {
    let d = net8();
    assert_eq!(d.class_name, "Form1");
    assert_eq!(d.style, Style { this: false, qualified: false });
    assert_eq!(framework().style, Style { this: true, qualified: true });
    let names: Vec<&str> = d.components().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Form1", "button1", "groupBox1", "checkBox1", "comboBox1", "timer1", "menuStrip1", "fileToolStripMenuItem"]);
    let root: Vec<&str> = d.root().children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(root, ["comboBox1", "groupBox1", "button1", "menuStrip1"]);
    assert_eq!(d.parent("checkBox1"), Some(("groupBox1", "")));
    assert_eq!(d.text_of("Form1").as_deref(), Some("Main Window"));
    assert_eq!(d.size("Form1"), (800, 450));
    assert_eq!(d.location("button1"), (713, 415));
    assert_eq!(d.anchor("button1"), ["Bottom", "Right"]);
    assert_eq!(d.events("button1"), [("Click", "button1_Click")]);
    assert_eq!(d.events("Form1"), [("Load", "Form1_Load")]);
    assert_eq!(d.list_items("comboBox1"), ["One", "Two"]);
    assert_eq!(d.strip_items("menuStrip1"), ["fileToolStripMenuItem"]);
    assert_eq!(d.dock("menuStrip1"), "Top", "a MenuStrip docks at the top without saying so");
    let tray: Vec<&str> = d.tray().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(tray, ["timer1"], "menu items aren't in the tray");
    let old = framework();
    assert_eq!(old.events("label1"), [("Click", "label1_Click")]);
    assert_eq!(parse_font(old.prop("label1", "Font").unwrap()).unwrap().size, 12.0);
}

#[test]
fn adds_controls_in_the_files_style() {
    let mut d = net8();
    let name = d.add_component("Button", None, (100, 200), None);
    assert_eq!(name, "button2");
    let text = d.to_text();
    assert!(text.contains("            button2 = new Button();\n            SuspendLayout();") || text.contains("button2 = new Button();\n            groupBox1.SuspendLayout();"), "{text}");
    assert!(text.contains(
        "            // \n            // button2\n            // \n            button2.Location = new Point(100, 200);\n            button2.Name = \"button2\";\n            button2.Size = new Size(75, 23);\n            button2.TabIndex = 4;\n            button2.Text = \"button2\";\n            button2.UseVisualStyleBackColor = true;\n            // \n            // Form1\n"
    ), "{text}");
    assert!(text.contains("            Controls.Add(button2);\n            Controls.Add(comboBox1);"), "new controls go to the front: {text}");
    assert!(text.contains("        private ToolStripMenuItem fileToolStripMenuItem;\n        private Button button2;\n    }"), "{text}");
    // Read back, it is the same model.
    assert_eq!(Designer::parse(&text, "Form").unwrap().to_text(), text);

    let mut old = framework();
    let t = old.add_component("Timer", None, (0, 0), None);
    let n = old.add_component("NumericUpDown", None, (10, 50), None);
    let text = old.to_text();
    assert!(text.contains("            this.components = new System.ComponentModel.Container();\n            this.label1 = new System.Windows.Forms.Label();"), "{text}");
    assert!(text.contains(&format!("this.{t} = new System.Windows.Forms.Timer(this.components);")), "{text}");
    assert!(text.contains(&format!("((System.ComponentModel.ISupportInitialize)(this.{n})).BeginInit();\n            this.SuspendLayout();")), "{text}");
    assert!(text.contains(&format!("((System.ComponentModel.ISupportInitialize)(this.{n})).EndInit();\n            this.ResumeLayout(false);")), "{text}");
    assert!(text.contains(&format!("this.{n}.Location = new System.Drawing.Point(10, 50);")), "{text}");
    assert!(text.contains(&format!("        private System.Windows.Forms.NumericUpDown {n};\n    }}")), "{text}");
}

#[test]
fn the_template_gets_layout_calls_and_fields() {
    let mut d = Designer::parse(TEMPLATE, "Form").unwrap();
    assert_eq!(d.to_text(), TEMPLATE, "no comment block is added to a block that had none");
    assert_eq!(d.style, Style { this: true, qualified: true });
    let b = d.add_component("Button", None, (8, 8), None);
    let text = d.to_text();
    assert!(text.contains(&format!("        this.{b} = new System.Windows.Forms.Button();\n        this.SuspendLayout();\n")), "{text}");
    assert!(text.contains("        this.ResumeLayout(false);\n        this.PerformLayout();\n    }\n"), "{text}");
    assert!(text.contains(&format!("    #endregion\n\n    private System.Windows.Forms.Button {b};\n}}\n")), "{text}");
    assert!(text.contains(&format!("        this.Controls.Add(this.{b});\n        this.Text = \"Form1\";")), "Controls sorts before Text: {text}");
}

#[test]
fn sets_and_clears_properties_and_events() {
    let mut d = net8();
    d.set_prop("button1", "BackColor", Some("Color.Red"));
    d.set_prop("button1", "Text", Some("\"Go\""));
    d.set_prop("button1", "TabIndex", None);
    d.set_event("button1", "MouseMove", Some("button1_MouseMove"));
    d.set_event("button1", "Click", None);
    let text = d.to_text();
    assert!(text.contains(
        "            button1.Anchor = AnchorStyles.Bottom | AnchorStyles.Right;\n            button1.BackColor = Color.Red;\n            button1.Location = new Point(713, 415);\n            button1.Name = \"button1\";\n            button1.Size = new Size(75, 23);\n            button1.Text = \"Go\";\n            button1.UseVisualStyleBackColor = true;\n            button1.MouseMove += button1_MouseMove;\n"
    ), "{text}");
    d.set_list_items("comboBox1", &["A \"quoted\"".into()]);
    assert_eq!(d.list_items("comboBox1"), ["A \"quoted\""]);
    d.set_list_items("comboBox1", &[]);
    assert!(!d.to_text().contains("comboBox1.Items"));

    let mut old = framework();
    old.set_event("MainForm", "Load", Some("MainForm_Load"));
    assert!(old.to_text().contains("            this.Load += new System.EventHandler(this.MainForm_Load);\n            ((System.ComponentModel"));
}

#[test]
fn removes_a_container_with_its_contents() {
    let mut d = net8();
    d.remove("groupBox1");
    d.remove("menuStrip1");
    let text = d.to_text();
    for gone in ["groupBox1", "checkBox1", "menuStrip1", "fileToolStripMenuItem"] {
        assert!(!text.contains(gone), "{gone} is gone: {text}");
    }
    assert!(text.contains("            Controls.Add(comboBox1);\n            Controls.Add(button1);\n            Name = \"Form1\";"), "MainMenuStrip went with the menu: {text}");
    assert_eq!(Designer::parse(&text, "Form").unwrap().to_text(), text);
}

#[test]
fn renames_everywhere() {
    let mut d = net8();
    d.rename("checkBox1", "rememberMe").unwrap();
    let text = d.to_text();
    assert!(!text.contains("checkBox1 ") && !text.contains("checkBox1;") && !text.contains("checkBox1."), "{text}");
    assert!(text.contains("            rememberMe = new CheckBox();"));
    assert!(text.contains("            groupBox1.Controls.Add(rememberMe);"));
    assert!(text.contains("            // rememberMe\n"));
    assert!(text.contains("            rememberMe.Name = \"rememberMe\";"));
    assert!(text.contains("            rememberMe.Text = \"checkBox1\";"), "the Text stays");
    assert!(text.contains("        private CheckBox rememberMe;"));
    assert!(d.rename("button1", "groupBox1").is_err());
    assert!(d.rename("button1", "2x").is_err());
    assert!(d.rename("Form1", "Other").is_err());
}

#[test]
fn moves_controls_between_containers() {
    let mut d = net8();
    d.reparent("button1", "groupBox1", "", (10, 50));
    assert_eq!(d.parent("button1"), Some(("groupBox1", "")));
    let text = d.to_text();
    assert!(text.contains("            groupBox1.Controls.Add(button1);\n            groupBox1.Controls.Add(checkBox1);"), "{text}");
    assert!(!text.contains("            Controls.Add(button1);"));
    d.reparent("groupBox1", "checkBox1", "", (0, 0));
    assert_eq!(d.parent("groupBox1"), Some(("Form1", "")), "a container can't go into itself");
    d.reorder("comboBox1", false);
    assert_eq!(d.root().children.last().map(|c| c.name.as_str()), Some("comboBox1"));
}

#[test]
fn resizing_moves_anchored_controls() {
    let mut d = net8();
    d.set_size("Form1", (900, 500));
    assert_eq!(d.size("Form1"), (900, 500));
    assert_eq!(d.location("button1"), (813, 465), "anchored bottom right, it keeps its distance to that corner");
    assert_eq!(d.location("groupBox1"), (12, 40));
}

#[test]
fn lays_out_docking_and_containers() {
    let d = net8();
    let l = layout(&d, &HashMap::new());
    assert_eq!(l.get("menuStrip1").unwrap().rect, Rect::new(0, 0, 800, 24));
    assert_eq!(l.get("checkBox1").unwrap().rect, Rect::new(18, 62, 83, 19), "inside the group box");
    assert_eq!(l.hit(20, 65).map(|p| p.name.as_str()), Some("checkBox1"));
    assert_eq!(l.hit(15, 45).map(|p| p.name.as_str()), Some("groupBox1"));
    assert_eq!(l.hit(500, 300), None);
    assert_eq!(l.area_at(&d, 100, 100, &[]).map(|a| a.owner.as_str()), Some("groupBox1"));
    assert_eq!(l.area_at(&d, 100, 100, &["groupBox1".into()]).map(|a| a.owner.as_str()), Some("Form1"));
    // Paint order: the group box before its check box, the menu (at the back) first.
    let order: Vec<&str> = l.controls.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(order, ["menuStrip1", "button1", "groupBox1", "checkBox1", "comboBox1"]);

    let old = framework();
    let l = layout(&old, &HashMap::new());
    assert_eq!(l.get("pictureBox1").unwrap().rect, Rect::new(0, 211, 284, 50));

    let mut tabs = net8();
    let tc = tabs.add_component("TabControl", None, (300, 100), None);
    let pages: Vec<String> = tabs.get(&tc).unwrap().children.iter().map(|c| c.name.clone()).collect();
    assert_eq!(pages, ["tabPage1", "tabPage2"]);
    let b = tabs.add_component("Button", Some("tabPage2"), (5, 5), None);
    let l = layout(&tabs, &HashMap::new());
    assert!(l.get("tabPage1").is_some() && l.get("tabPage2").is_none() && l.get(&b).is_none(), "only the first page shows");
    let l = layout(&tabs, &HashMap::from([(tc.clone(), 1)]));
    assert_eq!(l.get(&b).unwrap().rect, Rect::new(309, 129, 75, 23));
}

#[test]
fn other_root_types_and_errors() {
    let uc = TEMPLATE.replace("this.ClientSize = new System.Drawing.Size(800, 450);", "this.Size = new System.Drawing.Size(300, 200);");
    let d = Designer::parse(&uc, "UserControl").unwrap();
    assert_eq!(d.size("Form1"), (300, 200));
    assert!(Designer::parse("class A { }", "Form").is_err());
    assert!(Designer::parse("class A { void InitializeComponent() { x(); ", "Form").is_err());
}
