//! Linked editing of markup tags in the editor (see `EditorView::linked_edit`).

use super::*;

fn editor(cx: &mut gpui::TestAppContext, text: &str, file: &str) -> gpui::Entity<EditorView> {
    cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
    let path = PathBuf::from(file);
    cx.new(|cx| EditorView::new(text, Some(path), cx))
}

#[gpui::test]
fn renaming_a_tag_renames_its_closing_tag(cx: &mut gpui::TestAppContext) {
    let view = editor(cx, "<Project>\n  <ItemGroup>\n  </ItemGroup>\n</Project>\n", "App.csproj");
    view.update(cx, |v, cx| {
        // Caret after "Item" in the opening tag: typing goes into both names.
        v.state.set_caret(Point::new(1, 7), false);
        v.type_text("s", cx);
        assert_eq!(v.state.buffer.text(), "<Project>\n  <ItemsGroup>\n  </ItemsGroup>\n</Project>\n");
        assert_eq!(v.state.cursor_count(), 1, "the extra cursor is gone again");
        assert_eq!(v.state.caret(), Point::new(1, 8));
        // Backspace too.
        v.linked_edit(LinkedEdit::DeleteLeft, cx, |s| s.backspace(false));
        assert_eq!(v.state.buffer.text(), "<Project>\n  <ItemGroup>\n  </ItemGroup>\n</Project>\n");
        // Replacing the whole name, from the closing tag.
        v.state.select(Point::new(2, 4), Point::new(2, 13));
        v.type_text("PropertyGroup", cx);
        assert_eq!(v.state.buffer.text(), "<Project>\n  <PropertyGroup>\n  </PropertyGroup>\n</Project>\n");
    });
}

#[gpui::test]
fn typing_that_leaves_the_name_edits_one_tag(cx: &mut gpui::TestAppContext) {
    let view = editor(cx, "<div>\n</div>\n", "index.html");
    view.update(cx, |v, cx| {
        // A space after the name starts an attribute: only the opening tag changes.
        v.state.set_caret(Point::new(0, 4), false);
        v.type_text(" ", cx);
        assert_eq!(v.state.buffer.text(), "<div >\n</div>\n");
        // Backspace at the start of the name would delete the `<`: not linked.
        v.state.set_caret(Point::new(1, 2), false);
        v.linked_edit(LinkedEdit::DeleteLeft, cx, |s| s.backspace(false));
        assert_eq!(v.state.buffer.text(), "<div >\n<div>\n");
    });
    // Off: nothing is linked.
    let view = editor(cx, "<a>\n</a>\n", "x.xml");
    view.update(cx, |v, cx| {
        v.linked_editing = false;
        v.state.set_caret(Point::new(0, 2), false);
        v.type_text("b", cx);
        assert_eq!(v.state.buffer.text(), "<ab>\n</a>\n");
    });
}
