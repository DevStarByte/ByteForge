//! What the designer knows about Avalonia's controls: the toolbox, how each holds what is inside it, the
//! properties the grid offers and the events it can hook up.

/// How a property is edited in the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Text,
    Number,
    Bool,
    Enum(&'static [&'static str]),
    Brush,
    Thickness,
    /// RowDefinitions / ColumnDefinitions (`Auto,*,100`).
    Definitions,
}

#[derive(Debug)]
pub struct Prop {
    pub name: &'static str,
    pub category: &'static str,
    pub kind: Kind,
}

/// How a control holds what is written inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    /// Nothing (or only text).
    Leaf,
    /// One child, its Content (Button, Window, ScrollViewer, …).
    Content,
    /// One child (Border's Child).
    Decorator,
    /// Any number of children laid out by the panel.
    Panel,
    /// Items (ListBox, ComboBox, TabControl, Menu): any number of children shown as items.
    Items,
}

#[derive(Debug)]
pub struct Control {
    pub name: &'static str,
    /// Its toolbox group (empty: not in the toolbox).
    pub group: &'static str,
    pub role: Role,
    /// The XAML a new one is written as.
    pub snippet: &'static str,
    /// The size it gets when placed on a Canvas (where nothing stretches it).
    pub canvas_size: Option<(f32, f32)>,
    /// The property text written inside the element sets (`<Button>OK</Button>` is its Content).
    pub content_prop: &'static str,
    pub props: &'static [Prop],
    pub events: &'static [&'static str],
    /// Fluent's alignment for it (WinUI's: buttons and boxes don't stretch).
    pub h_align: &'static str,
    pub v_align: &'static str,
}

const H_ALIGN: &[&str] = &["Stretch", "Left", "Center", "Right"];
const V_ALIGN: &[&str] = &["Stretch", "Top", "Center", "Bottom"];
const ORIENTATION: &[&str] = &["Vertical", "Horizontal"];
const DOCK: &[&str] = &["Left", "Top", "Right", "Bottom"];
const TEXT_WRAPPING: &[&str] = &["NoWrap", "Wrap", "WrapWithOverflow"];
const TEXT_ALIGNMENT: &[&str] = &["Left", "Center", "Right", "Justify"];
const FONT_WEIGHT: &[&str] = &["Thin", "Light", "Normal", "Medium", "SemiBold", "Bold", "ExtraBold", "Black"];
const FONT_STYLE: &[&str] = &["Normal", "Italic", "Oblique"];
const STRETCH: &[&str] = &["None", "Fill", "Uniform", "UniformToFill"];
const SCROLL_BAR: &[&str] = &["Auto", "Visible", "Hidden", "Disabled"];
const THEME_VARIANT: &[&str] = &["Default", "Light", "Dark"];
const STARTUP: &[&str] = &["Manual", "CenterScreen", "CenterOwner"];
const SIZE_TO_CONTENT: &[&str] = &["Manual", "Width", "Height", "WidthAndHeight"];
const EXPAND_DIRECTION: &[&str] = &["Down", "Up", "Left", "Right"];
const TAB_PLACEMENT: &[&str] = &["Top", "Bottom", "Left", "Right"];
const SELECTION_MODE: &[&str] = &["Single", "Multiple", "Toggle", "AlwaysSelected"];

macro_rules! p {
    ($name:literal, $cat:literal, $kind:expr) => {
        Prop { name: $name, category: $cat, kind: $kind }
    };
}

/// The properties every control has.
pub const COMMON: &[Prop] = &[
    p!("Width", "Layout", Kind::Number),
    p!("Height", "Layout", Kind::Number),
    p!("MinWidth", "Layout", Kind::Number),
    p!("MinHeight", "Layout", Kind::Number),
    p!("MaxWidth", "Layout", Kind::Number),
    p!("MaxHeight", "Layout", Kind::Number),
    p!("Margin", "Layout", Kind::Thickness),
    p!("HorizontalAlignment", "Layout", Kind::Enum(H_ALIGN)),
    p!("VerticalAlignment", "Layout", Kind::Enum(V_ALIGN)),
    p!("IsVisible", "Appearance", Kind::Bool),
    p!("Opacity", "Appearance", Kind::Number),
    p!("IsEnabled", "Common", Kind::Bool),
    p!("ToolTip.Tip", "Common", Kind::Text),
    p!("Classes", "Common", Kind::Text),
];

/// The properties of controls with a template (TemplatedControl).
const TEMPLATED: &[Prop] = &[
    p!("Background", "Brush", Kind::Brush),
    p!("Foreground", "Brush", Kind::Brush),
    p!("BorderBrush", "Brush", Kind::Brush),
    p!("BorderThickness", "Appearance", Kind::Thickness),
    p!("CornerRadius", "Appearance", Kind::Thickness),
    p!("Padding", "Layout", Kind::Thickness),
    p!("FontSize", "Text", Kind::Number),
    p!("FontWeight", "Text", Kind::Enum(FONT_WEIGHT)),
    p!("FontStyle", "Text", Kind::Enum(FONT_STYLE)),
    p!("FontFamily", "Text", Kind::Text),
    p!("HorizontalContentAlignment", "Layout", Kind::Enum(H_ALIGN)),
    p!("VerticalContentAlignment", "Layout", Kind::Enum(V_ALIGN)),
];

const TEXT_PROPS: &[Prop] = &[
    p!("Text", "Common", Kind::Text),
    p!("Foreground", "Brush", Kind::Brush),
    p!("Background", "Brush", Kind::Brush),
    p!("FontSize", "Text", Kind::Number),
    p!("FontWeight", "Text", Kind::Enum(FONT_WEIGHT)),
    p!("FontStyle", "Text", Kind::Enum(FONT_STYLE)),
    p!("FontFamily", "Text", Kind::Text),
    p!("TextWrapping", "Text", Kind::Enum(TEXT_WRAPPING)),
    p!("TextAlignment", "Text", Kind::Enum(TEXT_ALIGNMENT)),
    p!("Padding", "Layout", Kind::Thickness),
];

const PANEL: &[Prop] = &[p!("Background", "Brush", Kind::Brush)];

/// Attached properties a child gets from its panel: (panel, properties).
pub const ATTACHED: &[(&str, &[Prop])] = &[
    ("Grid", &[p!("Grid.Row", "Layout", Kind::Number), p!("Grid.Column", "Layout", Kind::Number), p!("Grid.RowSpan", "Layout", Kind::Number), p!("Grid.ColumnSpan", "Layout", Kind::Number)]),
    ("DockPanel", &[p!("DockPanel.Dock", "Layout", Kind::Enum(DOCK))]),
    ("Canvas", &[p!("Canvas.Left", "Layout", Kind::Number), p!("Canvas.Top", "Layout", Kind::Number), p!("Canvas.Right", "Layout", Kind::Number), p!("Canvas.Bottom", "Layout", Kind::Number)]),
];

/// Every attached layout property (left behind when a control moves to another kind of panel).
pub const ATTACHED_NAMES: &[&str] = &["Grid.Row", "Grid.Column", "Grid.RowSpan", "Grid.ColumnSpan", "DockPanel.Dock", "Canvas.Left", "Canvas.Top", "Canvas.Right", "Canvas.Bottom"];

const BUTTON_EVENTS: &[&str] = &["Click", "PointerEntered", "PointerExited", "GotFocus", "LostFocus", "Loaded"];
const CONTROL_EVENTS: &[&str] = &["PointerPressed", "PointerReleased", "PointerEntered", "PointerExited", "KeyDown", "KeyUp", "GotFocus", "LostFocus", "Loaded", "SizeChanged"];
const TOGGLE_EVENTS: &[&str] = &["IsCheckedChanged", "Click", "Checked", "Unchecked", "Loaded"];
const SELECTOR_EVENTS: &[&str] = &["SelectionChanged", "DoubleTapped", "KeyDown", "GotFocus", "LostFocus", "Loaded"];
const VALUE_EVENTS: &[&str] = &["ValueChanged", "PointerReleased", "GotFocus", "LostFocus", "Loaded"];
const WINDOW_EVENTS: &[&str] = &["Opened", "Closing", "Closed", "Loaded", "Activated", "Deactivated", "KeyDown", "SizeChanged"];

#[allow(clippy::too_many_arguments)]
const fn c(
    name: &'static str,
    group: &'static str,
    role: Role,
    snippet: &'static str,
    canvas_size: Option<(f32, f32)>,
    content_prop: &'static str,
    props: &'static [Prop],
    events: &'static [&'static str],
) -> Control {
    Control { name, group, role, snippet, canvas_size, content_prop, props, events, h_align: "Stretch", v_align: "Stretch" }
}

const fn aligned(mut ctl: Control, h: &'static str, v: &'static str) -> Control {
    ctl.h_align = h;
    ctl.v_align = v;
    ctl
}

pub const LAYOUT: &str = "Layout";
pub const COMMON_GROUP: &str = "Common Controls";
pub const MORE: &str = "More Controls";
pub const SHAPES: &str = "Shapes";

pub static CONTROLS: &[Control] = &[
    // Layout
    c("Grid", LAYOUT, Role::Panel, r#"<Grid ColumnDefinitions="*,*" RowDefinitions="Auto,*"/>"#, Some((200., 120.)), "", &[
        p!("ColumnDefinitions", "Layout", Kind::Definitions),
        p!("RowDefinitions", "Layout", Kind::Definitions),
        p!("ShowGridLines", "Appearance", Kind::Bool),
        p!("Background", "Brush", Kind::Brush),
    ], CONTROL_EVENTS),
    c("StackPanel", LAYOUT, Role::Panel, "<StackPanel Spacing=\"4\"/>", Some((200., 120.)), "", &[
        p!("Orientation", "Layout", Kind::Enum(ORIENTATION)),
        p!("Spacing", "Layout", Kind::Number),
        p!("Background", "Brush", Kind::Brush),
    ], CONTROL_EVENTS),
    c("DockPanel", LAYOUT, Role::Panel, "<DockPanel/>", Some((200., 120.)), "", &[
        p!("LastChildFill", "Layout", Kind::Bool),
        p!("HorizontalSpacing", "Layout", Kind::Number),
        p!("VerticalSpacing", "Layout", Kind::Number),
        p!("Background", "Brush", Kind::Brush),
    ], CONTROL_EVENTS),
    c("WrapPanel", LAYOUT, Role::Panel, "<WrapPanel/>", Some((200., 120.)), "", &[
        p!("Orientation", "Layout", Kind::Enum(&["Horizontal", "Vertical"])),
        p!("ItemWidth", "Layout", Kind::Number),
        p!("ItemHeight", "Layout", Kind::Number),
        p!("Background", "Brush", Kind::Brush),
    ], CONTROL_EVENTS),
    c("UniformGrid", LAYOUT, Role::Panel, "<UniformGrid/>", Some((200., 120.)), "", &[
        p!("Rows", "Layout", Kind::Number),
        p!("Columns", "Layout", Kind::Number),
        p!("Background", "Brush", Kind::Brush),
    ], CONTROL_EVENTS),
    c("Canvas", LAYOUT, Role::Panel, "<Canvas/>", Some((200., 120.)), "", PANEL, CONTROL_EVENTS),
    c("Panel", LAYOUT, Role::Panel, "<Panel/>", Some((200., 120.)), "", PANEL, CONTROL_EVENTS),
    c("Border", LAYOUT, Role::Decorator, r#"<Border BorderBrush="Gray" BorderThickness="1" CornerRadius="4" Padding="8"/>"#, Some((120., 80.)), "", &[
        p!("Background", "Brush", Kind::Brush),
        p!("BorderBrush", "Brush", Kind::Brush),
        p!("BorderThickness", "Appearance", Kind::Thickness),
        p!("CornerRadius", "Appearance", Kind::Thickness),
        p!("Padding", "Layout", Kind::Thickness),
        p!("BoxShadow", "Appearance", Kind::Text),
    ], CONTROL_EVENTS),
    c("ScrollViewer", LAYOUT, Role::Content, "<ScrollViewer/>", Some((200., 120.)), "Content", &[
        p!("HorizontalScrollBarVisibility", "Layout", Kind::Enum(SCROLL_BAR)),
        p!("VerticalScrollBarVisibility", "Layout", Kind::Enum(SCROLL_BAR)),
        p!("Background", "Brush", Kind::Brush),
        p!("Padding", "Layout", Kind::Thickness),
    ], CONTROL_EVENTS),
    c("Expander", LAYOUT, Role::Content, r#"<Expander Header="Expander" IsExpanded="True"/>"#, Some((200., 120.)), "Content", &[
        p!("Header", "Common", Kind::Text),
        p!("IsExpanded", "Common", Kind::Bool),
        p!("ExpandDirection", "Layout", Kind::Enum(EXPAND_DIRECTION)),
    ], &["Expanded", "Collapsed", "Loaded"]),
    c("TabControl", LAYOUT, Role::Items, "<TabControl>\n<TabItem Header=\"Tab 1\"/>\n<TabItem Header=\"Tab 2\"/>\n</TabControl>", Some((240., 140.)), "", &[
        p!("SelectedIndex", "Common", Kind::Number),
        p!("TabStripPlacement", "Layout", Kind::Enum(TAB_PLACEMENT)),
    ], SELECTOR_EVENTS),
    c("TabItem", LAYOUT, Role::Content, r#"<TabItem Header="Tab"/>"#, None, "Content", &[p!("Header", "Common", Kind::Text), p!("IsSelected", "Common", Kind::Bool)], CONTROL_EVENTS),
    // Common controls
    aligned(c("Button", COMMON_GROUP, Role::Content, r#"<Button Content="Button"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("Command", "Common", Kind::Text),
        p!("IsDefault", "Common", Kind::Bool),
        p!("IsCancel", "Common", Kind::Bool),
    ], BUTTON_EVENTS), "Left", "Center"),
    c("TextBlock", COMMON_GROUP, Role::Leaf, r#"<TextBlock Text="TextBlock"/>"#, None, "Text", TEXT_PROPS, CONTROL_EVENTS),
    c("TextBox", COMMON_GROUP, Role::Leaf, "<TextBox/>", Some((160., 32.)), "Text", &[
        p!("Text", "Common", Kind::Text),
        p!("Watermark", "Common", Kind::Text),
        p!("IsReadOnly", "Common", Kind::Bool),
        p!("AcceptsReturn", "Common", Kind::Bool),
        p!("PasswordChar", "Common", Kind::Text),
        p!("MaxLength", "Common", Kind::Number),
        p!("TextWrapping", "Text", Kind::Enum(TEXT_WRAPPING)),
        p!("TextAlignment", "Text", Kind::Enum(TEXT_ALIGNMENT)),
    ], &["TextChanged", "KeyDown", "KeyUp", "GotFocus", "LostFocus", "Loaded"]),
    c("Label", COMMON_GROUP, Role::Content, r#"<Label Content="Label"/>"#, None, "Content", &[p!("Content", "Common", Kind::Text), p!("Target", "Common", Kind::Text)], CONTROL_EVENTS),
    aligned(c("CheckBox", COMMON_GROUP, Role::Content, r#"<CheckBox Content="CheckBox"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("IsChecked", "Common", Kind::Bool),
        p!("IsThreeState", "Common", Kind::Bool),
    ], TOGGLE_EVENTS), "Left", "Center"),
    aligned(c("RadioButton", COMMON_GROUP, Role::Content, r#"<RadioButton Content="RadioButton"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("IsChecked", "Common", Kind::Bool),
        p!("GroupName", "Common", Kind::Text),
    ], TOGGLE_EVENTS), "Left", "Center"),
    aligned(c("ToggleButton", COMMON_GROUP, Role::Content, r#"<ToggleButton Content="ToggleButton"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("IsChecked", "Common", Kind::Bool),
    ], TOGGLE_EVENTS), "Left", "Center"),
    aligned(c("ToggleSwitch", COMMON_GROUP, Role::Content, "<ToggleSwitch/>", None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("IsChecked", "Common", Kind::Bool),
        p!("OnContent", "Common", Kind::Text),
        p!("OffContent", "Common", Kind::Text),
    ], TOGGLE_EVENTS), "Left", "Center"),
    aligned(c("ComboBox", COMMON_GROUP, Role::Items, "<ComboBox PlaceholderText=\"Select\">\n<ComboBoxItem>Item 1</ComboBoxItem>\n<ComboBoxItem>Item 2</ComboBoxItem>\n</ComboBox>", None, "", &[
        p!("SelectedIndex", "Common", Kind::Number),
        p!("PlaceholderText", "Common", Kind::Text),
        p!("IsDropDownOpen", "Common", Kind::Bool),
        p!("MaxDropDownHeight", "Layout", Kind::Number),
    ], SELECTOR_EVENTS), "Left", "Top"),
    c("ListBox", COMMON_GROUP, Role::Items, "<ListBox>\n<ListBoxItem>Item 1</ListBoxItem>\n<ListBoxItem>Item 2</ListBoxItem>\n</ListBox>", Some((200., 120.)), "", &[
        p!("SelectedIndex", "Common", Kind::Number),
        p!("SelectionMode", "Common", Kind::Enum(SELECTION_MODE)),
    ], SELECTOR_EVENTS),
    c("Slider", COMMON_GROUP, Role::Leaf, r#"<Slider Minimum="0" Maximum="100" Value="50"/>"#, Some((160., 32.)), "", &[
        p!("Minimum", "Common", Kind::Number),
        p!("Maximum", "Common", Kind::Number),
        p!("Value", "Common", Kind::Number),
        p!("Orientation", "Layout", Kind::Enum(&["Horizontal", "Vertical"])),
        p!("TickFrequency", "Common", Kind::Number),
        p!("IsSnapToTickEnabled", "Common", Kind::Bool),
    ], VALUE_EVENTS),
    c("ProgressBar", COMMON_GROUP, Role::Leaf, r#"<ProgressBar Value="50"/>"#, Some((160., 8.)), "", &[
        p!("Minimum", "Common", Kind::Number),
        p!("Maximum", "Common", Kind::Number),
        p!("Value", "Common", Kind::Number),
        p!("IsIndeterminate", "Common", Kind::Bool),
        p!("ShowProgressText", "Common", Kind::Bool),
        p!("Orientation", "Layout", Kind::Enum(&["Horizontal", "Vertical"])),
        p!("Foreground", "Brush", Kind::Brush),
    ], VALUE_EVENTS),
    c("Image", COMMON_GROUP, Role::Leaf, r#"<Image Width="64" Height="64"/>"#, None, "", &[p!("Source", "Common", Kind::Text), p!("Stretch", "Layout", Kind::Enum(STRETCH))], CONTROL_EVENTS),
    c("Separator", COMMON_GROUP, Role::Leaf, "<Separator/>", Some((160., 9.)), "", &[], &[]),
    // More controls
    c("NumericUpDown", MORE, Role::Leaf, r#"<NumericUpDown Value="0"/>"#, Some((160., 32.)), "", &[
        p!("Value", "Common", Kind::Number),
        p!("Minimum", "Common", Kind::Number),
        p!("Maximum", "Common", Kind::Number),
        p!("Increment", "Common", Kind::Number),
        p!("FormatString", "Common", Kind::Text),
        p!("Watermark", "Common", Kind::Text),
    ], &["ValueChanged", "GotFocus", "LostFocus", "Loaded"]),
    aligned(c("DatePicker", MORE, Role::Leaf, "<DatePicker/>", None, "", &[p!("Header", "Common", Kind::Text), p!("SelectedDate", "Common", Kind::Text)], &["SelectedDateChanged", "Loaded"]), "Left", "Center"),
    aligned(c("TimePicker", MORE, Role::Leaf, "<TimePicker/>", None, "", &[p!("Header", "Common", Kind::Text), p!("SelectedTime", "Common", Kind::Text)], &["SelectedTimeChanged", "Loaded"]), "Left", "Center"),
    aligned(c("Calendar", MORE, Role::Leaf, "<Calendar/>", None, "", &[p!("SelectedDate", "Common", Kind::Text), p!("DisplayDate", "Common", Kind::Text)], &["SelectedDatesChanged", "Loaded"]), "Left", "Top"),
    aligned(c("HyperlinkButton", MORE, Role::Content, r#"<HyperlinkButton Content="Link" NavigateUri="https://avaloniaui.net"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("NavigateUri", "Common", Kind::Text),
    ], BUTTON_EVENTS), "Left", "Center"),
    aligned(c("RepeatButton", MORE, Role::Content, r#"<RepeatButton Content="Repeat"/>"#, None, "Content", &[
        p!("Content", "Common", Kind::Text),
        p!("Interval", "Common", Kind::Number),
        p!("Delay", "Common", Kind::Number),
    ], BUTTON_EVENTS), "Left", "Center"),
    c("SelectableTextBlock", MORE, Role::Leaf, r#"<SelectableTextBlock Text="Selectable text"/>"#, None, "Text", TEXT_PROPS, CONTROL_EVENTS),
    c("MaskedTextBox", MORE, Role::Leaf, r#"<MaskedTextBox Mask="(000) 000-0000"/>"#, Some((160., 32.)), "Text", &[p!("Mask", "Common", Kind::Text), p!("Text", "Common", Kind::Text)], &["TextChanged", "Loaded"]),
    c("Menu", MORE, Role::Items, "<Menu>\n<MenuItem Header=\"_File\"/>\n<MenuItem Header=\"_Edit\"/>\n</Menu>", Some((240., 32.)), "", &[], &["Opened", "Closed", "Loaded"]),
    c("MenuItem", MORE, Role::Items, r#"<MenuItem Header="Item"/>"#, None, "", &[p!("Header", "Common", Kind::Text), p!("InputGesture", "Common", Kind::Text), p!("Command", "Common", Kind::Text)], &["Click", "SubmenuOpened"]),
    c("ListBoxItem", MORE, Role::Content, "<ListBoxItem>Item</ListBoxItem>", None, "Content", &[p!("Content", "Common", Kind::Text), p!("IsSelected", "Common", Kind::Bool)], CONTROL_EVENTS),
    c("ComboBoxItem", MORE, Role::Content, "<ComboBoxItem>Item</ComboBoxItem>", None, "Content", &[p!("Content", "Common", Kind::Text), p!("IsSelected", "Common", Kind::Bool)], CONTROL_EVENTS),
    // Shapes
    c("Rectangle", SHAPES, Role::Leaf, r#"<Rectangle Width="80" Height="50" Fill="CornflowerBlue"/>"#, None, "", &[
        p!("Fill", "Brush", Kind::Brush),
        p!("Stroke", "Brush", Kind::Brush),
        p!("StrokeThickness", "Appearance", Kind::Number),
        p!("RadiusX", "Appearance", Kind::Number),
        p!("RadiusY", "Appearance", Kind::Number),
    ], CONTROL_EVENTS),
    c("Ellipse", SHAPES, Role::Leaf, r#"<Ellipse Width="60" Height="60" Fill="Orange"/>"#, None, "", &[
        p!("Fill", "Brush", Kind::Brush),
        p!("Stroke", "Brush", Kind::Brush),
        p!("StrokeThickness", "Appearance", Kind::Number),
    ], CONTROL_EVENTS),
    // Roots (not in the toolbox)
    c("Window", "", Role::Content, "", None, "Content", &[
        p!("Title", "Common", Kind::Text),
        p!("Icon", "Common", Kind::Text),
        p!("Background", "Brush", Kind::Brush),
        p!("Foreground", "Brush", Kind::Brush),
        p!("Padding", "Layout", Kind::Thickness),
        p!("FontSize", "Text", Kind::Number),
        p!("FontFamily", "Text", Kind::Text),
        p!("d:DesignWidth", "Design", Kind::Number),
        p!("d:DesignHeight", "Design", Kind::Number),
        p!("RequestedThemeVariant", "Appearance", Kind::Enum(THEME_VARIANT)),
        p!("WindowStartupLocation", "Layout", Kind::Enum(STARTUP)),
        p!("SizeToContent", "Layout", Kind::Enum(SIZE_TO_CONTENT)),
        p!("CanResize", "Common", Kind::Bool),
        p!("ShowInTaskbar", "Common", Kind::Bool),
        p!("Topmost", "Common", Kind::Bool),
    ], WINDOW_EVENTS),
    c("UserControl", "", Role::Content, "", None, "Content", &[
        p!("Background", "Brush", Kind::Brush),
        p!("Foreground", "Brush", Kind::Brush),
        p!("Padding", "Layout", Kind::Thickness),
        p!("FontSize", "Text", Kind::Number),
        p!("d:DesignWidth", "Design", Kind::Number),
        p!("d:DesignHeight", "Design", Kind::Number),
    ], CONTROL_EVENTS),
    c("ContentControl", "", Role::Content, "<ContentControl/>", None, "Content", TEMPLATED, CONTROL_EVENTS),
    c("Viewbox", "", Role::Decorator, "<Viewbox/>", None, "", &[p!("Stretch", "Layout", Kind::Enum(STRETCH))], CONTROL_EVENTS),
    c("RelativePanel", "", Role::Panel, "<RelativePanel/>", None, "", PANEL, CONTROL_EVENTS),
];

/// The control of a tag (`Button`, `controls:Button`); `None` for ones the designer doesn't know.
pub fn control(tag: &str) -> Option<&'static Control> {
    let local = local_name(tag);
    CONTROLS.iter().find(|c| c.name == local)
}

/// A tag without its namespace prefix.
pub fn local_name(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

/// The toolbox: groups in order, each with its controls.
pub fn toolbox() -> Vec<(&'static str, Vec<&'static Control>)> {
    let mut groups: Vec<(&'static str, Vec<&'static Control>)> = Vec::new();
    for c in CONTROLS.iter().filter(|c| !c.group.is_empty()) {
        match groups.iter_mut().find(|(g, _)| *g == c.group) {
            Some((_, v)) => v.push(c),
            None => groups.push((c.group, vec![c])),
        }
    }
    groups
}

/// The properties the grid shows for a control of `tag` inside a `parent_tag` (its attached ones first).
pub fn properties(tag: &str, parent_tag: Option<&str>) -> Vec<&'static Prop> {
    let mut out: Vec<&'static Prop> = Vec::new();
    if let Some(parent) = parent_tag.map(local_name)
        && let Some((_, attached)) = ATTACHED.iter().find(|(p, _)| *p == parent)
    {
        out.extend(attached.iter());
    }
    let own = control(tag);
    if let Some(c) = own {
        out.extend(c.props.iter());
    }
    // Templated controls (not panels, shapes or text blocks) have brushes, fonts and padding.
    let templated = own.is_some_and(|c| matches!(c.role, Role::Content | Role::Items | Role::Leaf) && !matches!(c.name, "TextBlock" | "SelectableTextBlock" | "Image" | "Separator" | "Window" | "UserControl"));
    if templated {
        for p in TEMPLATED {
            if !out.iter().any(|o| o.name == p.name) {
                out.push(p);
            }
        }
    }
    for p in COMMON {
        if !out.iter().any(|o| o.name == p.name) {
            out.push(p);
        }
    }
    out
}

/// The events the grid offers for a control.
pub fn events(tag: &str) -> &'static [&'static str] {
    control(tag).map_or(CONTROL_EVENTS, |c| c.events)
}

/// The type of an event's arguments and the namespace it is in.
pub fn event_args(event: &str) -> (&'static str, &'static str) {
    match event {
        "TextChanged" => ("TextChangedEventArgs", "Avalonia.Controls"),
        "SelectionChanged" => ("SelectionChangedEventArgs", "Avalonia.Controls"),
        "ValueChanged" => ("RangeBaseValueChangedEventArgs", "Avalonia.Controls.Primitives"),
        "PointerPressed" => ("PointerPressedEventArgs", "Avalonia.Input"),
        "PointerReleased" => ("PointerReleasedEventArgs", "Avalonia.Input"),
        "PointerEntered" | "PointerExited" | "PointerMoved" => ("PointerEventArgs", "Avalonia.Input"),
        "KeyDown" | "KeyUp" => ("KeyEventArgs", "Avalonia.Input"),
        "GotFocus" => ("GotFocusEventArgs", "Avalonia.Input"),
        "DoubleTapped" | "Tapped" => ("TappedEventArgs", "Avalonia.Input"),
        "SizeChanged" => ("SizeChangedEventArgs", "Avalonia.Controls"),
        "Closing" => ("WindowClosingEventArgs", "Avalonia.Controls"),
        "Opened" | "Closed" | "Activated" | "Deactivated" => ("EventArgs", "System"),
        "SelectedDateChanged" => ("DatePickerSelectedValueChangedEventArgs", "Avalonia.Controls"),
        "SelectedTimeChanged" => ("TimePickerSelectedValueChangedEventArgs", "Avalonia.Controls"),
        "SelectedDatesChanged" => ("SelectionChangedEventArgs", "Avalonia.Controls"),
        _ => ("RoutedEventArgs", "Avalonia.Interactivity"),
    }
}

/// The event a double click hooks up.
pub fn default_event(tag: &str) -> &'static str {
    match local_name(tag) {
        "Window" => "Opened",
        "TextBox" | "MaskedTextBox" => "TextChanged",
        "CheckBox" | "RadioButton" | "ToggleButton" | "ToggleSwitch" => "IsCheckedChanged",
        "ComboBox" | "ListBox" | "TabControl" => "SelectionChanged",
        "Slider" | "NumericUpDown" | "ProgressBar" => "ValueChanged",
        "DatePicker" => "SelectedDateChanged",
        "TimePicker" => "SelectedTimeChanged",
        "Button" | "RepeatButton" | "HyperlinkButton" | "MenuItem" => "Click",
        _ => "Loaded",
    }
}
