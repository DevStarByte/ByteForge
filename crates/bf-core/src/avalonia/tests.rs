use std::collections::HashMap;

use super::*;

const WINDOW: &str = r#"<Window xmlns="https://github.com/avaloniaui"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        xmlns:d="http://schemas.microsoft.com/expression/blend/2008"
        xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
        mc:Ignorable="d" d:DesignWidth="400" d:DesignHeight="300"
        x:Class="App.Views.MainWindow"
        Title="App">
    <!-- the form -->
    <StackPanel Margin="10" Spacing="4">
        <TextBlock Text="Hello"/>
        <Button Content="OK" Click="Ok_Click"/>
        <Button>Cancel</Button>
    </StackPanel>
</Window>
"#;

/// Every character is 7 pixels wide.
fn measure(text: &str, _size: f32, _bold: bool) -> f32 {
    text.chars().count() as f32 * 7.0
}

fn layout(doc: &Doc) -> Layout {
    Layout::of(doc, &measure, &HashMap::new())
}

#[test]
fn reads_the_tree() {
    let doc = Doc::parse(WINDOW).unwrap();
    assert_eq!(doc.root.tag, "Window");
    assert_eq!(doc.class_name(), Some("MainWindow"));
    assert_eq!(doc.indent, "    ");
    let panel = doc.get(&[0]).unwrap();
    assert_eq!((panel.tag.as_str(), panel.children.len(), panel.line), ("StackPanel", 3, 8));
    assert_eq!(doc.get(&[0, 1]).unwrap().attr("Click"), Some("Ok_Click"));
    let cancel = doc.get(&[0, 2]).unwrap();
    assert_eq!(cancel.value("Content").as_deref(), Some("Cancel"));
    assert_eq!(cancel.label(), "Button \"Cancel\"");
    assert_eq!(doc.at_line(10), Some(vec![0, 1]));
    assert_eq!(doc.at_line(8), Some(vec![0]));
}

#[test]
fn sets_and_removes_attributes_in_place() {
    let doc = Doc::parse(WINDOW).unwrap();
    let t = doc.set_attr(&[0, 1], "Content", Some("Yes & no")).unwrap();
    assert!(t.contains(r#"<Button Content="Yes &amp; no" Click="Ok_Click"/>"#), "{t}");
    let t = doc.set_attr(&[0, 1], "Width", Some("80")).unwrap();
    assert!(t.contains(r#"<Button Content="OK" Click="Ok_Click" Width="80"/>"#), "{t}");
    let t = doc.set_attr(&[0, 1], "x:Name", Some("ok")).unwrap();
    assert!(t.contains(r#"<Button x:Name="ok" Content="OK" Click="Ok_Click"/>"#), "{t}");
    let t = doc.set_attr(&[0, 1], "Click", None).unwrap();
    assert!(t.contains(r#"<Button Content="OK"/>"#), "{t}");
    // The text inside an element is its content.
    let t = doc.set_attr(&[0, 2], "Content", Some("Stop")).unwrap();
    assert!(t.contains("<Button>Stop</Button>"), "{t}");
    // Attributes on lines of their own: a new one gets its own line too.
    let t = doc.set_attr(&[], "Width", Some("500")).unwrap();
    assert!(t.contains("        Title=\"App\"\n        Width=\"500\">"), "{t}");
    let t = Doc::parse(&t).unwrap().set_attr(&[], "Title", None).unwrap();
    assert!(t.contains("x:Class=\"App.Views.MainWindow\"\n        Width=\"500\">"), "{t}");
    // Nothing else changed.
    assert!(t.contains("    <!-- the form -->\n"));
}

#[test]
fn inserts_removes_and_moves_elements() {
    let doc = Doc::parse(WINDOW).unwrap();
    let t = doc.insert(&[0], 1, "<CheckBox/>").unwrap();
    assert!(t.contains("        <TextBlock Text=\"Hello\"/>\n        <CheckBox/>\n        <Button Content=\"OK\""), "{t}");
    let t = doc.insert(&[0], 3, "<CheckBox/>").unwrap();
    assert!(t.contains("        <Button>Cancel</Button>\n        <CheckBox/>\n    </StackPanel>"), "{t}");
    // Into an empty element: it opens.
    let d = Doc::parse(&doc.insert(&[0], 3, "<Border/>").unwrap()).unwrap();
    let t = d.insert(&[0, 3], 0, &indent_snippet("<StackPanel>\n<Button/>\n</StackPanel>", &d.indent)).unwrap();
    assert!(t.contains("        <Border>\n            <StackPanel>\n                <Button/>\n            </StackPanel>\n        </Border>\n"), "{t}");
    // Removed with its line.
    let t = doc.remove(&[vec![0, 1]]).unwrap();
    assert!(t.contains("        <TextBlock Text=\"Hello\"/>\n        <Button>Cancel</Button>\n"), "{t}");
    // Moved to the front: the same element, nothing else changed.
    let drop = Drop { parent: vec![0], index: 0, attrs: vec![], in_place: false };
    let (t, path) = doc.move_to(&[0, 2], &drop).unwrap();
    assert_eq!(path, vec![0, 0]);
    assert!(t.contains("    <StackPanel Margin=\"10\" Spacing=\"4\">\n        <Button>Cancel</Button>\n        <TextBlock"), "{t}");
    // Moved down past its neighbour.
    let drop = Drop { parent: vec![0], index: 2, attrs: vec![], in_place: false };
    let (t, path) = doc.move_to(&[0, 0], &drop).unwrap();
    assert_eq!(path, vec![0, 1]);
    assert_eq!(Doc::parse(&t).unwrap().get(&[0, 1]).unwrap().attr("Text"), Some("Hello"));
    assert!(doc.move_to(&[0], &Drop { parent: vec![0, 1], index: 0, attrs: vec![], in_place: false }).is_err());
    // Copied source keeps its inner indentation.
    let d = Doc::parse(&doc.insert(&[0], 0, &indent_snippet("<Border>\n<TextBlock/>\n</Border>", "    ")).unwrap()).unwrap();
    assert_eq!(d.source(&[0, 0]).unwrap(), "<Border>\n    <TextBlock/>\n</Border>");
    assert!(d.accepts("<Button x:Name=\"b\"/>"));
    assert!(!d.accepts("<a:Button/>"));
    assert!(!d.accepts("hello"));
}

#[test]
fn paths_follow_edits() {
    assert_eq!(path_after_removal(&[0, 1], &[0, 2, 0]), Some(vec![0, 1, 0]));
    assert_eq!(path_after_removal(&[0, 1], &[0, 0]), Some(vec![0, 0]));
    assert_eq!(path_after_removal(&[0, 1], &[0, 1, 3]), None);
    assert_eq!(path_after_insert(&[0, 1], &[0, 1]), vec![0, 2]);
    assert_eq!(path_after_insert(&[0, 1], &[1]), vec![1]);
}

#[test]
fn values() {
    assert_eq!(parse_color("#f00"), Some(0xff0000ff));
    assert_eq!(parse_color("#80ff0000"), Some(0xff000080));
    assert_eq!(parse_color("#123456"), Some(0x123456ff));
    assert_eq!(parse_color("CornflowerBlue"), Some(0x6495edff));
    assert_eq!(parse_thickness("1,2"), Some([1.0, 2.0, 1.0, 2.0]));
    assert_eq!(parse_thickness("1 2 3 4"), Some([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(parse_grid_lengths("Auto,*,2*,100"), vec![GridLen::Auto, GridLen::Star(1.0), GridLen::Star(2.0), GridLen::Px(100.0)]);
    assert!(is_markup("{Binding Name}"));
    assert!(!is_markup("{}{literal}"));
    assert_eq!(format_num(12.0), "12");
    assert_eq!(format_num(12.5), "12.5");
}

#[test]
fn lays_out_a_stack() {
    let doc = Doc::parse(WINDOW).unwrap();
    let l = layout(&doc);
    assert_eq!(l.size, (400.0, 300.0));
    let panel = l.get(&[0]).unwrap().rect;
    assert_eq!(panel, Rect::new(10.0, 10.0, 380.0, 280.0));
    // The text stretches; buttons keep their size, on the left (Fluent).
    let text = l.get(&[0, 0]).unwrap().rect;
    assert_eq!((text.x, text.y, text.w, text.h), (10.0, 10.0, 380.0, 19.0));
    let ok = l.get(&[0, 1]).unwrap().rect;
    assert_eq!((ok.x, ok.y, ok.w, ok.h), (10.0, 33.0, 2.0 * 7.0 + 18.0, 32.0));
    let cancel = l.get(&[0, 2]).unwrap().rect;
    assert_eq!((cancel.y, cancel.w), (69.0, 6.0 * 7.0 + 18.0));
    assert_eq!(l.hit(15.0, 40.0).unwrap().path, vec![0, 1]);
    assert_eq!(l.hit(300.0, 40.0).unwrap().path, vec![0]);
    assert_eq!(l.hit(5.0, 5.0).unwrap().path, Vec::<usize>::new());
}

#[test]
fn lays_out_grids_docks_and_canvases() {
    let text = r#"<UserControl xmlns="https://github.com/avaloniaui" Width="300" Height="200">
  <Grid ColumnDefinitions="100,*" RowDefinitions="Auto,*">
    <TextBlock Text="Name"/>
    <TextBox Grid.Column="1"/>
    <DockPanel Grid.Row="1" Grid.ColumnSpan="2">
      <Border DockPanel.Dock="Top" Height="20"/>
      <Canvas>
        <Rectangle Canvas.Left="10" Canvas.Top="5" Width="30" Height="30"/>
      </Canvas>
    </DockPanel>
  </Grid>
</UserControl>"#;
    let doc = Doc::parse(text).unwrap();
    assert_eq!(doc.indent, "  ");
    let l = layout(&doc);
    assert_eq!(l.get(&[0, 1]).unwrap().rect, Rect::new(100.0, 0.0, 200.0, 32.0));
    assert_eq!(l.get(&[0, 2]).unwrap().rect, Rect::new(0.0, 32.0, 300.0, 168.0));
    assert_eq!(l.get(&[0, 2, 0]).unwrap().rect, Rect::new(0.0, 32.0, 300.0, 20.0));
    assert_eq!(l.get(&[0, 2, 1, 0]).unwrap().rect, Rect::new(10.0, 57.0, 30.0, 30.0));
    let Some(Area { kind: AreaKind::Grid { cols, rows }, .. }) = l.area(&[0]) else { panic!("the grid's area") };
    assert_eq!((cols.as_slice(), rows.as_slice()), (&[0.0, 100.0, 300.0][..], &[0.0, 32.0, 200.0][..]));

    // Dropping a new control in the grid's second row, first column; on the canvas, where the mouse is.
    let plan = l.drop_at(&doc, 50.0, 100.0, None, "Button").unwrap();
    assert_eq!(plan.drop.parent, vec![0, 2, 1]);
    let plan = l.drop_at(&doc, 50.0, 40.0, None, "Button").unwrap();
    assert_eq!(plan.drop.parent, vec![0, 2, 0], "the empty border");
    let plan = l.drop_at(&doc, 20.0, 20.0, None, "Button").unwrap();
    assert_eq!((plan.drop.parent.as_slice(), plan.drop.attrs.as_slice()), (&[0usize][..], &[][..]));
    let plan = l.drop_at(&doc, 150.0, 10.0, None, "Button").unwrap();
    assert_eq!(plan.drop.attrs, vec![("Grid.Column".to_owned(), Some("1".to_owned()))]);
    let plan = l.drop_at(&doc, 100.0, 100.0, None, "Button").unwrap();
    assert_eq!(plan.drop.attrs, vec![("Canvas.Left".to_owned(), Some("100".to_owned())), ("Canvas.Top".to_owned(), Some("48".to_owned()))]);
    // The rectangle moved on its canvas keeps its place among the children; out to the grid it loses its
    // canvas position and gets a cell.
    let plan = l.drop_at(&doc, 60.0, 80.0, Some((&[0, 2, 1, 0], (5.0, 5.0))), "Rectangle").unwrap();
    assert!(plan.drop.in_place);
    let (t, path) = doc.move_to(&[0, 2, 1, 0], &plan.drop).unwrap();
    assert_eq!(path, vec![0, 2, 1, 0]);
    assert!(t.contains(r#"<Rectangle Canvas.Left="55" Canvas.Top="23" Width="30" Height="30"/>"#), "{t}");
    let plan = l.drop_at(&doc, 150.0, 10.0, Some((&[0, 2, 1, 0], (0.0, 0.0))), "Rectangle").unwrap();
    let (t, path) = doc.move_to(&[0, 2, 1, 0], &plan.drop).unwrap();
    assert_eq!(path, vec![0, 3]);
    assert!(t.contains("    <Rectangle Width=\"30\" Height=\"30\" Grid.Column=\"1\"/>\n  </Grid>"), "{t}");
    assert!(t.contains("      <Canvas>\n      </Canvas>"), "{t}");
}

#[test]
fn drops_into_stacks_between_children() {
    let doc = Doc::parse(WINDOW).unwrap();
    let l = layout(&doc);
    // Between the text block (10..29) and OK (33..65).
    let plan = l.drop_at(&doc, 100.0, 31.0, None, "CheckBox").unwrap();
    assert_eq!((plan.drop.parent.as_slice(), plan.drop.index), (&[0usize][..], 1));
    assert!(plan.line.is_some());
    let (t, path) = doc.insert_at(&plan.drop, "<CheckBox/>").unwrap();
    assert_eq!(path, vec![0, 1]);
    assert!(t.contains("<TextBlock Text=\"Hello\"/>\n        <CheckBox/>"), "{t}");
    // The window holds the stack panel: nothing else goes into it.
    let plan = l.drop_at(&doc, 5.0, 5.0, None, "Button");
    assert!(plan.is_none());
}

#[test]
fn lays_out_tabs_and_lists() {
    let text = r#"<UserControl xmlns="https://github.com/avaloniaui" Width="300" Height="200">
    <TabControl>
        <TabItem Header="One">
            <TextBlock Text="Page one"/>
        </TabItem>
        <TabItem Header="Two">
            <ListBox>
                <ListBoxItem>A</ListBoxItem>
                <TextBlock Text="B"/>
            </ListBox>
        </TabItem>
    </TabControl>
</UserControl>"#;
    let doc = Doc::parse(text).unwrap();
    let l = layout(&doc);
    assert!(l.get(&[0, 0, 0]).is_some(), "the first page shows");
    assert!(l.get(&[0, 1, 0]).is_none());
    let tabs: HashMap<ElPath, usize> = [(vec![0], 1)].into();
    let l = Layout::of(&doc, &measure, &tabs);
    assert!(l.get(&[0, 0, 0]).is_none());
    let a = l.get(&[0, 1, 0, 0]).unwrap().rect;
    let b = l.get(&[0, 1, 0, 1]).unwrap().rect;
    assert_eq!(a.h, 19.0 + 21.0);
    assert_eq!((b.x - a.x, b.y - a.bottom()), (12.0, 9.0));
    // A new tab item goes onto the tab strip; anything else into the page shown (when it's empty).
    let plan = l.drop_at(&doc, 250.0, 10.0, None, "TabItem").unwrap();
    assert_eq!((plan.drop.parent.as_slice(), plan.drop.index), (&[0usize][..], 2));
    assert!(l.drop_at(&doc, 150.0, 150.0, None, "Button").is_some_and(|p| p.drop.parent == vec![0, 1, 0]));
}
