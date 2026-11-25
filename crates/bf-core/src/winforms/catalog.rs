//! What the designer knows about Windows Forms types: the toolbox, each control's default size, initial
//! properties and default event, and the properties and events the property grid offers.

/// A type the toolbox offers (or the designer knows how to draw).
#[derive(Debug)]
pub struct ControlInfo {
    pub type_name: &'static str,
    pub group: &'static str,
    /// The size a new one gets.
    pub size: (i32, i32),
    /// Properties a new one gets besides Location, Name, Size and TabIndex, as code (`{text}` is its name).
    pub initial: &'static [(&'static str, &'static str)],
    /// The event a double click creates a handler for.
    pub default_event: &'static str,
    /// Holds other controls.
    pub container: bool,
    /// Not a control: shown in the component tray below the form.
    pub component: bool,
    /// Implements ISupportInitialize (BeginInit/EndInit around its setup).
    pub init: bool,
    /// Created with the form's components container (`new Timer(components)`).
    pub needs_container: bool,
    /// Its Dock when the code doesn't set one.
    pub dock: &'static str,
    /// Gets a Text property.
    pub text: bool,
    /// An item of a strip, a menu or a grid (`Items.AddRange`, `DropDownItems.AddRange`, `Columns.AddRange`):
    /// not a control of its own.
    pub item: bool,
}

const fn control(type_name: &'static str, group: &'static str, size: (i32, i32), default_event: &'static str) -> ControlInfo {
    ControlInfo { type_name, group, size, initial: &[], default_event, container: false, component: false, init: false, needs_container: false, dock: "None", text: false, item: false }
}

const fn item(type_name: &'static str, group: &'static str, default_event: &'static str) -> ControlInfo {
    ControlInfo { item: true, ..control(type_name, group, (0, 0), default_event) }
}

pub const COMMON: &str = "Common Controls";
pub const CONTAINERS: &str = "Containers";
pub const MENUS: &str = "Menus & Toolbars";
pub const DATA: &str = "Data";
pub const COMPONENTS: &str = "Components";

/// The toolbox, in the order it lists them.
pub static TOOLBOX: &[ControlInfo] = &[
    ControlInfo { initial: &[("Text", "{text}"), ("UseVisualStyleBackColor", "true")], text: true, ..control("Button", COMMON, (75, 23), "Click") },
    ControlInfo { initial: &[("AutoSize", "true"), ("Text", "{text}"), ("UseVisualStyleBackColor", "true")], text: true, ..control("CheckBox", COMMON, (83, 19), "CheckedChanged") },
    ControlInfo { initial: &[("FormattingEnabled", "true")], ..control("CheckedListBox", COMMON, (120, 94), "SelectedIndexChanged") },
    ControlInfo { initial: &[("FormattingEnabled", "true")], ..control("ComboBox", COMMON, (121, 23), "SelectedIndexChanged") },
    control("DateTimePicker", COMMON, (200, 23), "ValueChanged"),
    ControlInfo { initial: &[("AutoSize", "true"), ("Text", "{text}")], text: true, ..control("Label", COMMON, (38, 15), "Click") },
    ControlInfo { initial: &[("AutoSize", "true"), ("TabStop", "true"), ("Text", "{text}")], text: true, ..control("LinkLabel", COMMON, (60, 15), "LinkClicked") },
    ControlInfo { initial: &[("FormattingEnabled", "true"), ("ItemHeight", "15")], ..control("ListBox", COMMON, (120, 94), "SelectedIndexChanged") },
    ControlInfo { init: true, ..control("NumericUpDown", COMMON, (120, 23), "ValueChanged") },
    ControlInfo { init: true, ..control("PictureBox", COMMON, (100, 50), "Click") },
    control("ProgressBar", COMMON, (100, 23), "Click"),
    ControlInfo { initial: &[("AutoSize", "true"), ("TabStop", "true"), ("Text", "{text}"), ("UseVisualStyleBackColor", "true")], text: true, ..control("RadioButton", COMMON, (94, 19), "CheckedChanged") },
    ControlInfo { initial: &[("Text", "\"\"")], ..control("RichTextBox", COMMON, (100, 96), "TextChanged") },
    control("TextBox", COMMON, (100, 23), "TextChanged"),
    ControlInfo { init: true, ..control("TrackBar", COMMON, (104, 45), "Scroll") },
    ControlInfo { container: true, ..control("FlowLayoutPanel", CONTAINERS, (200, 100), "Paint") },
    ControlInfo { container: true, initial: &[("Text", "{text}")], text: true, ..control("GroupBox", CONTAINERS, (200, 100), "Enter") },
    ControlInfo { container: true, ..control("Panel", CONTAINERS, (200, 100), "Paint") },
    ControlInfo { container: true, init: true, ..control("SplitContainer", CONTAINERS, (150, 100), "SplitterMoved") },
    ControlInfo { container: true, initial: &[("SelectedIndex", "0")], ..control("TabControl", CONTAINERS, (200, 100), "SelectedIndexChanged") },
    ControlInfo { container: true, initial: &[("ColumnCount", "2"), ("RowCount", "2")], ..control("TableLayoutPanel", CONTAINERS, (200, 100), "Paint") },
    ControlInfo { initial: &[("Text", "{text}")], dock: "Top", text: true, ..control("MenuStrip", MENUS, (800, 24), "ItemClicked") },
    ControlInfo { initial: &[("Text", "{text}")], dock: "Bottom", text: true, ..control("StatusStrip", MENUS, (800, 22), "ItemClicked") },
    ControlInfo { initial: &[("Text", "{text}")], dock: "Top", text: true, ..control("ToolStrip", MENUS, (800, 25), "ItemClicked") },
    ControlInfo { init: true, initial: &[("ColumnHeadersHeightSizeMode", "DataGridViewColumnHeadersHeightSizeMode.AutoSize")], ..control("DataGridView", DATA, (240, 150), "CellContentClick") },
    ControlInfo { component: true, needs_container: true, ..control("Timer", COMPONENTS, (0, 0), "Tick") },
    ControlInfo { component: true, needs_container: true, ..control("ToolTip", COMPONENTS, (0, 0), "Popup") },
    ControlInfo { component: true, needs_container: true, init: true, ..control("ErrorProvider", COMPONENTS, (0, 0), "") },
    ControlInfo { component: true, initial: &[("FileName", "{text}")], ..control("OpenFileDialog", COMPONENTS, (0, 0), "FileOk") },
    ControlInfo { component: true, ..control("SaveFileDialog", COMPONENTS, (0, 0), "FileOk") },
    ControlInfo { component: true, ..control("FolderBrowserDialog", COMPONENTS, (0, 0), "") },
    ControlInfo { component: true, ..control("ColorDialog", COMPONENTS, (0, 0), "") },
];

/// Types the toolbox doesn't offer but that forms contain and the designer draws.
static OTHER: &[ControlInfo] = &[
    ControlInfo { container: true, text: true, initial: &[("Text", "{text}"), ("UseVisualStyleBackColor", "true")], ..control("TabPage", CONTAINERS, (192, 72), "Click") },
    ControlInfo { container: true, text: true, ..control("Form", CONTAINERS, (800, 450), "Load") },
    ControlInfo { container: true, ..control("UserControl", CONTAINERS, (150, 150), "Load") },
    ControlInfo { container: true, ..control("SplitterPanel", CONTAINERS, (0, 0), "Paint") },
    ControlInfo { text: true, ..item("ToolStripMenuItem", MENUS, "Click") },
    ControlInfo { text: true, ..item("ToolStripStatusLabel", MENUS, "Click") },
    ControlInfo { text: true, ..item("ToolStripButton", MENUS, "Click") },
    ControlInfo { text: true, ..item("ToolStripLabel", MENUS, "Click") },
    item("ToolStripSeparator", MENUS, "Paint"),
    item("DataGridViewTextBoxColumn", DATA, "Disposed"),
    item("DataGridViewCheckBoxColumn", DATA, "Disposed"),
    item("DataGridViewComboBoxColumn", DATA, "Disposed"),
    item("DataGridViewButtonColumn", DATA, "Disposed"),
    item("DataGridViewLinkColumn", DATA, "Disposed"),
    item("DataGridViewImageColumn", DATA, "Disposed"),
    ControlInfo { component: true, needs_container: true, ..control("ContextMenuStrip", MENUS, (0, 0), "Opening") },
    ControlInfo { component: true, needs_container: true, init: true, ..control("BindingSource", DATA, (0, 0), "CurrentChanged") },
    ControlInfo { component: true, needs_container: true, ..control("NotifyIcon", COMPONENTS, (0, 0), "MouseDoubleClick") },
    ControlInfo { component: true, ..control("FontDialog", COMPONENTS, (0, 0), "Apply") },
    ControlInfo { component: true, ..control("ImageList", COMPONENTS, (0, 0), "") },
    ControlInfo { text: true, ..control("Control", COMMON, (100, 23), "Click") },
];

/// The type's name without its namespace.
pub fn short_type(type_name: &str) -> &str {
    type_name.rsplit('.').next().unwrap_or(type_name)
}

pub fn info(type_name: &str) -> Option<&'static ControlInfo> {
    let short = short_type(type_name);
    TOOLBOX.iter().chain(OTHER).find(|c| c.type_name == short)
}

/// Whether instances of the type are components in the tray (not controls on the form).
pub fn is_component(type_name: &str) -> bool {
    match info(type_name) {
        Some(i) => i.component,
        // Unknown types: dialogs, providers and the like end up in the tray, the rest are controls.
        None => {
            let s = short_type(type_name);
            ["Dialog", "Provider", "Source", "List", "Timer", "Icon", "Component", "Worker", "Adapter"].iter().any(|e| s.ends_with(e))
        }
    }
}

/// Whether the type is an item of a strip, menu or grid (see [`ControlInfo::item`]).
pub fn is_item_type(type_name: &str) -> bool {
    info(type_name).is_some_and(|i| i.item)
}

/// The collection a strip, menu item or grid keeps its items in, and the type of its array:
/// `("Items", "ToolStripItem")`, `("DropDownItems", "ToolStripItem")`, `("Columns", "DataGridViewColumn")`.
pub fn item_collection(owner_type: &str) -> Option<(&'static str, &'static str)> {
    match short_type(owner_type) {
        "MenuStrip" | "ToolStrip" | "StatusStrip" | "ContextMenuStrip" => Some(("Items", "ToolStripItem")),
        "ToolStripMenuItem" | "ToolStripDropDownButton" | "ToolStripSplitButton" => Some(("DropDownItems", "ToolStripItem")),
        "DataGridView" => Some(("Columns", "DataGridViewColumn")),
        _ => None,
    }
}

/// The item a strip, menu item or grid gets when one is added: a menu item in menus, a button on a tool
/// strip, a label on a status strip, a text column in a grid.
pub fn default_item_type(owner_type: &str) -> Option<&'static str> {
    match short_type(owner_type) {
        "MenuStrip" | "ContextMenuStrip" | "ToolStripMenuItem" | "ToolStripDropDownButton" | "ToolStripSplitButton" => Some("ToolStripMenuItem"),
        "ToolStrip" => Some("ToolStripButton"),
        "StatusStrip" => Some("ToolStripStatusLabel"),
        "DataGridView" => Some("DataGridViewTextBoxColumn"),
        _ => None,
    }
}

/// The column types a grid's toolbar offers: (type, label).
pub const COLUMN_TYPES: &[(&str, &str)] = &[
    ("DataGridViewTextBoxColumn", "Text"),
    ("DataGridViewCheckBoxColumn", "Check Box"),
    ("DataGridViewComboBoxColumn", "Combo Box"),
    ("DataGridViewButtonColumn", "Button"),
    ("DataGridViewLinkColumn", "Link"),
    ("DataGridViewImageColumn", "Image"),
];

pub fn default_event(type_name: &str) -> &'static str {
    info(type_name).map(|i| i.default_event).filter(|e| !e.is_empty()).unwrap_or("Click")
}

/// How a property is edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropKind {
    Text,
    Int,
    Bool,
    Decimal,
    Char,
    /// An enum value: (enum type, members).
    Enum(&'static str, &'static [&'static str]),
    Point,
    Size,
    Color,
    Font,
    Anchor,
    /// The items of a list (`Items.AddRange(new object[] { … })`), one per line.
    Items,
    /// A component of the form (AcceptButton = button1).
    Reference,
    /// Code, as written.
    Code,
}

#[derive(Debug)]
pub struct PropInfo {
    pub name: &'static str,
    pub kind: PropKind,
    /// The value that means "not set" (the property is then left out of the code), as the grid shows it.
    pub default: Option<&'static str>,
    pub category: &'static str,
    /// The types that have it (empty: every control).
    pub types: &'static [&'static str],
    /// Only the form (root) has it.
    pub root: bool,
}

const fn prop(name: &'static str, kind: PropKind, category: &'static str) -> PropInfo {
    PropInfo { name, kind, default: None, category, types: &[], root: false }
}

const APPEARANCE: &str = "Appearance";
const BEHAVIOR: &str = "Behavior";
const LAYOUT: &str = "Layout";
const WINDOW: &str = "Window Style";

const DOCK: &[&str] = &["None", "Top", "Bottom", "Left", "Right", "Fill"];
const BORDER: &[&str] = &["None", "FixedSingle", "Fixed3D"];
const ALIGN: &[&str] = &["TopLeft", "TopCenter", "TopRight", "MiddleLeft", "MiddleCenter", "MiddleRight", "BottomLeft", "BottomCenter", "BottomRight"];
const TEXT_TYPES: &[&str] = &[
    "Button", "CheckBox", "Label", "LinkLabel", "RadioButton", "GroupBox", "TabPage", "TextBox", "ComboBox", "RichTextBox", "MenuStrip", "StatusStrip", "ToolStrip",
    "ToolStripMenuItem", "ToolStripStatusLabel", "ToolStripButton", "ToolStripLabel", "Control",
];
const STRIP_ITEMS: &[&str] = &["ToolStripMenuItem", "ToolStripButton", "ToolStripStatusLabel", "ToolStripLabel", "ToolStripSeparator"];
const COLUMNS: &[&str] = &["DataGridViewTextBoxColumn", "DataGridViewCheckBoxColumn", "DataGridViewComboBoxColumn", "DataGridViewButtonColumn", "DataGridViewLinkColumn", "DataGridViewImageColumn"];

/// Every property the grid knows, in the grid's order within a category.
pub static PROPERTIES: &[PropInfo] = &[
    // The form
    PropInfo { root: true, ..prop("Text", PropKind::Text, APPEARANCE) },
    PropInfo { root: true, default: Some("Sizable"), ..prop("FormBorderStyle", PropKind::Enum("FormBorderStyle", &["None", "FixedSingle", "Fixed3D", "FixedDialog", "Sizable", "FixedToolWindow", "SizableToolWindow"]), APPEARANCE) },
    PropInfo { root: true, ..prop("ClientSize", PropKind::Size, LAYOUT) },
    PropInfo { root: true, default: Some("WindowsDefaultLocation"), ..prop("StartPosition", PropKind::Enum("FormStartPosition", &["Manual", "CenterScreen", "WindowsDefaultLocation", "WindowsDefaultBounds", "CenterParent"]), LAYOUT) },
    PropInfo { root: true, default: Some("Normal"), ..prop("WindowState", PropKind::Enum("FormWindowState", &["Normal", "Minimized", "Maximized"]), LAYOUT) },
    PropInfo { root: true, default: Some("0, 0"), ..prop("MinimumSize", PropKind::Size, LAYOUT) },
    PropInfo { root: true, default: Some("0, 0"), ..prop("MaximumSize", PropKind::Size, LAYOUT) },
    PropInfo { root: true, default: Some("true"), ..prop("MaximizeBox", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("true"), ..prop("MinimizeBox", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("true"), ..prop("ControlBox", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("true"), ..prop("ShowIcon", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("true"), ..prop("ShowInTaskbar", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("false"), ..prop("TopMost", PropKind::Bool, WINDOW) },
    PropInfo { root: true, default: Some("false"), ..prop("KeyPreview", PropKind::Bool, BEHAVIOR) },
    PropInfo { root: true, default: Some(""), ..prop("AcceptButton", PropKind::Reference, BEHAVIOR) },
    PropInfo { root: true, default: Some(""), ..prop("CancelButton", PropKind::Reference, BEHAVIOR) },
    PropInfo { root: true, default: Some(""), ..prop("BackColor", PropKind::Color, APPEARANCE) },
    PropInfo { root: true, default: Some(""), ..prop("ForeColor", PropKind::Color, APPEARANCE) },
    PropInfo { root: true, default: Some(""), ..prop("Font", PropKind::Font, APPEARANCE) },
    // Every control
    PropInfo { types: TEXT_TYPES, ..prop("Text", PropKind::Text, APPEARANCE) },
    // Items of strips and menus
    PropInfo { default: Some("false"), types: &["ToolStripMenuItem", "ToolStripButton"], ..prop("Checked", PropKind::Bool, APPEARANCE) },
    PropInfo { default: Some("false"), types: &["ToolStripMenuItem", "ToolStripButton"], ..prop("CheckOnClick", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["ToolStripMenuItem"], ..prop("ShortcutKeys", PropKind::Code, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["ToolStripMenuItem", "ToolStripButton", "ToolStripStatusLabel", "ToolStripLabel"], ..prop("ToolTipText", PropKind::Text, BEHAVIOR) },
    PropInfo { default: Some("ImageAndText"), types: &["ToolStripButton"], ..prop("DisplayStyle", PropKind::Enum("ToolStripItemDisplayStyle", &["None", "Text", "Image", "ImageAndText"]), APPEARANCE) },
    PropInfo { default: Some("false"), types: &["ToolStripStatusLabel"], ..prop("Spring", PropKind::Bool, LAYOUT) },
    PropInfo { default: Some("true"), types: STRIP_ITEMS, ..prop("Enabled", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("true"), types: STRIP_ITEMS, ..prop("Visible", PropKind::Bool, BEHAVIOR) },
    // Columns of grids
    PropInfo { default: Some(""), types: COLUMNS, ..prop("HeaderText", PropKind::Text, APPEARANCE) },
    PropInfo { default: Some(""), types: COLUMNS, ..prop("DataPropertyName", PropKind::Text, DATA_CATEGORY) },
    PropInfo { default: Some("100"), types: COLUMNS, ..prop("Width", PropKind::Int, LAYOUT) },
    PropInfo { default: Some("5"), types: COLUMNS, ..prop("MinimumWidth", PropKind::Int, LAYOUT) },
    PropInfo {
        default: Some("NotSet"),
        types: COLUMNS,
        ..prop(
            "AutoSizeMode",
            PropKind::Enum("DataGridViewAutoSizeColumnMode", &["NotSet", "None", "AllCellsExceptHeader", "AllCells", "DisplayedCellsExceptHeader", "DisplayedCells", "ColumnHeader", "Fill"]),
            LAYOUT,
        )
    },
    PropInfo { default: Some("false"), types: COLUMNS, ..prop("ReadOnly", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("false"), types: COLUMNS, ..prop("Frozen", PropKind::Bool, LAYOUT) },
    PropInfo { default: Some("true"), types: COLUMNS, ..prop("Visible", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("DropDownButton"), types: &["DataGridViewComboBoxColumn"], ..prop("DisplayStyle", PropKind::Enum("DataGridViewComboBoxDisplayStyle", &["ComboBox", "DropDownButton", "Nothing"]), APPEARANCE) },
    PropInfo { default: Some(""), types: &["DataGridViewButtonColumn", "DataGridViewLinkColumn"], ..prop("Text", PropKind::Text, APPEARANCE) },
    PropInfo { default: Some(""), ..prop("BackColor", PropKind::Color, APPEARANCE) },
    PropInfo { default: Some(""), ..prop("ForeColor", PropKind::Color, APPEARANCE) },
    PropInfo { default: Some(""), ..prop("Font", PropKind::Font, APPEARANCE) },
    PropInfo { default: Some("MiddleCenter"), types: &["Button"], ..prop("TextAlign", PropKind::Enum("ContentAlignment", ALIGN), APPEARANCE) },
    PropInfo { default: Some("TopLeft"), types: &["Label", "LinkLabel"], ..prop("TextAlign", PropKind::Enum("ContentAlignment", ALIGN), APPEARANCE) },
    PropInfo { default: Some("Left"), types: &["TextBox"], ..prop("TextAlign", PropKind::Enum("HorizontalAlignment", &["Left", "Right", "Center"]), APPEARANCE) },
    PropInfo { default: Some("Standard"), types: &["Button", "CheckBox", "RadioButton", "Label", "GroupBox", "ComboBox"], ..prop("FlatStyle", PropKind::Enum("FlatStyle", &["Flat", "Popup", "Standard", "System"]), APPEARANCE) },
    PropInfo { default: Some("None"), types: &["Label", "Panel", "PictureBox", "FlowLayoutPanel", "TableLayoutPanel", "SplitContainer"], ..prop("BorderStyle", PropKind::Enum("BorderStyle", BORDER), APPEARANCE) },
    PropInfo { default: Some("Fixed3D"), types: &["TextBox", "RichTextBox", "ListBox", "CheckedListBox", "DataGridView", "NumericUpDown"], ..prop("BorderStyle", PropKind::Enum("BorderStyle", BORDER), APPEARANCE) },
    PropInfo { default: Some(""), types: &["TextBox"], ..prop("PlaceholderText", PropKind::Text, APPEARANCE) },
    PropInfo { types: &["Button", "CheckBox", "RadioButton", "TabPage"], ..prop("UseVisualStyleBackColor", PropKind::Bool, APPEARANCE) },
    PropInfo { default: Some("false"), types: &["CheckBox", "RadioButton"], ..prop("Checked", PropKind::Bool, APPEARANCE) },
    PropInfo { default: Some("false"), types: &["TextBox", "RichTextBox"], ..prop("ReadOnly", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("false"), types: &["TextBox"], ..prop("Multiline", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("false"), types: &["TextBox"], ..prop("UseSystemPasswordChar", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["TextBox"], ..prop("PasswordChar", PropKind::Char, BEHAVIOR) },
    PropInfo { default: Some("None"), types: &["TextBox"], ..prop("ScrollBars", PropKind::Enum("ScrollBars", &["None", "Horizontal", "Vertical", "Both"]), APPEARANCE) },
    PropInfo { default: Some("32767"), types: &["TextBox"], ..prop("MaxLength", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("DropDown"), types: &["ComboBox"], ..prop("DropDownStyle", PropKind::Enum("ComboBoxStyle", &["Simple", "DropDown", "DropDownList"]), APPEARANCE) },
    PropInfo { default: Some(""), types: &["ComboBox", "ListBox", "CheckedListBox"], ..prop("Items", PropKind::Items, DATA_CATEGORY) },
    PropInfo { default: Some("false"), types: &["ListBox", "ComboBox", "CheckedListBox"], ..prop("Sorted", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("0"), types: &["NumericUpDown"], ..prop("Value", PropKind::Decimal, APPEARANCE) },
    PropInfo { default: Some("0"), types: &["NumericUpDown"], ..prop("Minimum", PropKind::Decimal, DATA_CATEGORY) },
    PropInfo { default: Some("100"), types: &["NumericUpDown"], ..prop("Maximum", PropKind::Decimal, DATA_CATEGORY) },
    PropInfo { default: Some("1"), types: &["NumericUpDown"], ..prop("Increment", PropKind::Decimal, DATA_CATEGORY) },
    PropInfo { default: Some("0"), types: &["NumericUpDown"], ..prop("DecimalPlaces", PropKind::Int, DATA_CATEGORY) },
    PropInfo { default: Some("0"), types: &["ProgressBar", "TrackBar"], ..prop("Value", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("0"), types: &["ProgressBar", "TrackBar"], ..prop("Minimum", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("100"), types: &["ProgressBar"], ..prop("Maximum", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("10"), types: &["TrackBar"], ..prop("Maximum", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("1"), types: &["TrackBar"], ..prop("TickFrequency", PropKind::Int, APPEARANCE) },
    PropInfo { default: Some("Blocks"), types: &["ProgressBar"], ..prop("Style", PropKind::Enum("ProgressBarStyle", &["Blocks", "Continuous", "Marquee"]), BEHAVIOR) },
    PropInfo { default: Some("Long"), types: &["DateTimePicker"], ..prop("Format", PropKind::Enum("DateTimePickerFormat", &["Long", "Short", "Time", "Custom"]), APPEARANCE) },
    PropInfo { default: Some(""), types: &["DateTimePicker"], ..prop("CustomFormat", PropKind::Text, BEHAVIOR) },
    PropInfo { default: Some("Normal"), types: &["PictureBox"], ..prop("SizeMode", PropKind::Enum("PictureBoxSizeMode", &["Normal", "StretchImage", "AutoSize", "CenterImage", "Zoom"]), BEHAVIOR) },
    PropInfo { default: Some("LeftToRight"), types: &["FlowLayoutPanel"], ..prop("FlowDirection", PropKind::Enum("FlowDirection", &["LeftToRight", "TopDown", "RightToLeft", "BottomUp"]), LAYOUT) },
    PropInfo { default: Some("true"), types: &["FlowLayoutPanel"], ..prop("WrapContents", PropKind::Bool, LAYOUT) },
    PropInfo { default: Some("false"), types: &["Panel", "FlowLayoutPanel", "TableLayoutPanel", "TabPage"], ..prop("AutoScroll", PropKind::Bool, LAYOUT) },
    PropInfo { default: Some("0"), types: &["TableLayoutPanel"], ..prop("ColumnCount", PropKind::Int, LAYOUT) },
    PropInfo { default: Some("0"), types: &["TableLayoutPanel"], ..prop("RowCount", PropKind::Int, LAYOUT) },
    PropInfo { default: Some("Vertical"), types: &["SplitContainer"], ..prop("Orientation", PropKind::Enum("Orientation", &["Horizontal", "Vertical"]), LAYOUT) },
    PropInfo { types: &["SplitContainer"], ..prop("SplitterDistance", PropKind::Int, LAYOUT) },
    PropInfo { default: Some("-1"), types: &["TabControl"], ..prop("SelectedIndex", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("true"), types: &["DataGridView"], ..prop("AllowUserToAddRows", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("true"), types: &["DataGridView"], ..prop("AllowUserToDeleteRows", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("false"), types: &["DataGridView"], ..prop("ReadOnly", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("None"), types: &["Button"], ..prop("DialogResult", PropKind::Enum("DialogResult", &["None", "OK", "Cancel", "Abort", "Retry", "Ignore", "Yes", "No"]), BEHAVIOR) },
    PropInfo { default: Some("false"), types: &["Timer"], ..prop("Enabled", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("100"), types: &["Timer"], ..prop("Interval", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["OpenFileDialog", "SaveFileDialog"], ..prop("FileName", PropKind::Text, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["OpenFileDialog", "SaveFileDialog"], ..prop("Filter", PropKind::Text, BEHAVIOR) },
    PropInfo { default: Some(""), types: &["OpenFileDialog", "SaveFileDialog", "FolderBrowserDialog"], ..prop("Title", PropKind::Text, BEHAVIOR) },
    PropInfo { default: Some("true"), ..prop("Enabled", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("true"), ..prop("Visible", PropKind::Bool, BEHAVIOR) },
    PropInfo { default: Some("false"), types: &["Button", "CheckBox", "Label", "LinkLabel", "RadioButton", "GroupBox", "Panel", "FlowLayoutPanel", "TableLayoutPanel", "TextBox"], ..prop("AutoSize", PropKind::Bool, LAYOUT) },
    PropInfo { default: Some("None"), ..prop("Dock", PropKind::Enum("DockStyle", DOCK), LAYOUT) },
    PropInfo { default: Some("Top, Left"), ..prop("Anchor", PropKind::Anchor, LAYOUT) },
    PropInfo { default: Some("0, 0"), ..prop("Location", PropKind::Point, LAYOUT) },
    prop("Size", PropKind::Size, LAYOUT),
    PropInfo { default: Some("0"), ..prop("TabIndex", PropKind::Int, BEHAVIOR) },
    PropInfo { default: Some("true"), ..prop("TabStop", PropKind::Bool, BEHAVIOR) },
];

const DATA_CATEGORY: &str = "Data";

/// Properties that only make sense on controls (not on tray components like a Timer).
const CONTROL_ONLY: &[&str] = &["BackColor", "ForeColor", "Font", "Visible", "Dock", "Anchor", "Location", "Size", "TabIndex", "TabStop", "Enabled"];

/// The properties the grid shows for a component of this type (`root`: the form itself).
pub fn properties_for(type_name: &str, root: bool) -> Vec<&'static PropInfo> {
    let short = short_type(type_name);
    let component = !root && is_component(type_name);
    let item = !root && is_item_type(type_name);
    let mut out: Vec<&'static PropInfo> = Vec::new();
    for p in PROPERTIES {
        let fits = if item {
            // Items have only what is listed for them: no Location, Dock, TabIndex.
            p.types.contains(&short)
        } else if root {
            p.root || (p.types.is_empty() && matches!(p.name, "Enabled" | "AutoScroll"))
        } else if p.root {
            false
        } else if p.types.is_empty() {
            !component || !CONTROL_ONLY.contains(&p.name)
        } else {
            p.types.contains(&short)
        };
        if fits && !out.iter().any(|o| o.name == p.name) {
            out.push(p);
        }
    }
    if root && short == "UserControl" {
        out.retain(|p| !p.root || matches!(p.name, "BackColor" | "ForeColor" | "Font"));
        out.push(&USER_CONTROL_SIZE);
    }
    // Components: their Enabled (a Timer's) is in the type list above.
    if component {
        out.retain(|p| p.name != "Enabled" || p.types.contains(&short));
    }
    out
}

static USER_CONTROL_SIZE: PropInfo = prop("Size", PropKind::Size, LAYOUT);

pub fn property(type_name: &str, root: bool, name: &str) -> Option<&'static PropInfo> {
    properties_for(type_name, root).into_iter().find(|p| p.name == name)
}

/// An event: its name, the type of its EventArgs, and its delegate type (for code that names it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventInfo {
    pub name: &'static str,
    pub args: &'static str,
    pub delegate: &'static str,
}

const fn ev(name: &'static str, args: &'static str, delegate: &'static str) -> EventInfo {
    EventInfo { name, args, delegate }
}

static EVENTS: &[EventInfo] = &[
    ev("Click", "EventArgs", "System.EventHandler"),
    ev("DoubleClick", "EventArgs", "System.EventHandler"),
    ev("TextChanged", "EventArgs", "System.EventHandler"),
    ev("CheckedChanged", "EventArgs", "System.EventHandler"),
    ev("SelectedIndexChanged", "EventArgs", "System.EventHandler"),
    ev("ValueChanged", "EventArgs", "System.EventHandler"),
    ev("Scroll", "EventArgs", "System.EventHandler"),
    ev("Load", "EventArgs", "System.EventHandler"),
    ev("Shown", "EventArgs", "System.EventHandler"),
    ev("Enter", "EventArgs", "System.EventHandler"),
    ev("Leave", "EventArgs", "System.EventHandler"),
    ev("Resize", "EventArgs", "System.EventHandler"),
    ev("Tick", "EventArgs", "System.EventHandler"),
    ev("Popup", "PopupEventArgs", "System.Windows.Forms.PopupEventHandler"),
    ev("CurrentChanged", "EventArgs", "System.EventHandler"),
    ev("FormClosing", "FormClosingEventArgs", "System.Windows.Forms.FormClosingEventHandler"),
    ev("FormClosed", "FormClosedEventArgs", "System.Windows.Forms.FormClosedEventHandler"),
    ev("KeyDown", "KeyEventArgs", "System.Windows.Forms.KeyEventHandler"),
    ev("KeyUp", "KeyEventArgs", "System.Windows.Forms.KeyEventHandler"),
    ev("KeyPress", "KeyPressEventArgs", "System.Windows.Forms.KeyPressEventHandler"),
    ev("MouseClick", "MouseEventArgs", "System.Windows.Forms.MouseEventHandler"),
    ev("MouseDoubleClick", "MouseEventArgs", "System.Windows.Forms.MouseEventHandler"),
    ev("MouseDown", "MouseEventArgs", "System.Windows.Forms.MouseEventHandler"),
    ev("MouseUp", "MouseEventArgs", "System.Windows.Forms.MouseEventHandler"),
    ev("MouseMove", "MouseEventArgs", "System.Windows.Forms.MouseEventHandler"),
    ev("Paint", "PaintEventArgs", "System.Windows.Forms.PaintEventHandler"),
    ev("Validating", "System.ComponentModel.CancelEventArgs", "System.ComponentModel.CancelEventHandler"),
    ev("Opening", "System.ComponentModel.CancelEventArgs", "System.ComponentModel.CancelEventHandler"),
    ev("FileOk", "System.ComponentModel.CancelEventArgs", "System.ComponentModel.CancelEventHandler"),
    ev("CellContentClick", "DataGridViewCellEventArgs", "System.Windows.Forms.DataGridViewCellEventHandler"),
    ev("CellValueChanged", "DataGridViewCellEventArgs", "System.Windows.Forms.DataGridViewCellEventHandler"),
    ev("LinkClicked", "LinkLabelLinkClickedEventArgs", "System.Windows.Forms.LinkLabelLinkClickedEventHandler"),
    ev("ItemClicked", "ToolStripItemClickedEventArgs", "System.Windows.Forms.ToolStripItemClickedEventHandler"),
    ev("SplitterMoved", "SplitterEventArgs", "System.Windows.Forms.SplitterEventHandler"),
    ev("Apply", "EventArgs", "System.EventHandler"),
];

/// What an event's handler takes (unknown events: plain EventArgs).
pub fn event_info(name: &str) -> EventInfo {
    EVENTS.iter().find(|e| e.name == name).copied().unwrap_or(EventInfo { name: "", args: "EventArgs", delegate: "System.EventHandler" })
}

/// The events the grid lists for a type: its default event first, then the common ones.
pub fn events_for(type_name: &str, root: bool) -> Vec<&'static str> {
    let short = short_type(type_name);
    let mut out: Vec<&'static str> = Vec::new();
    let mut add = |e: &'static str| {
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    };
    if root {
        for e in ["Load", "Shown", "FormClosing", "FormClosed", "Resize", "KeyDown", "KeyPress", "Paint", "Click"] {
            add(e);
        }
        return out;
    }
    if is_item_type(type_name) {
        if STRIP_ITEMS.contains(&short) && short != "ToolStripSeparator" {
            add("Click");
            if matches!(short, "ToolStripMenuItem" | "ToolStripButton") {
                add("CheckedChanged");
            }
            if short == "ToolStripMenuItem" {
                add("DropDownOpening");
            }
        }
        return out;
    }
    add(default_event(type_name));
    if is_component(type_name) {
        return out;
    }
    let specific: &[&'static str] = match short {
        "TextBox" | "RichTextBox" => &["TextChanged", "KeyDown", "KeyPress", "Validating"],
        "ComboBox" => &["SelectedIndexChanged", "TextChanged"],
        "DataGridView" => &["CellContentClick", "CellValueChanged"],
        "CheckBox" | "RadioButton" => &["CheckedChanged"],
        _ => &[],
    };
    for e in specific {
        add(e);
    }
    for e in ["Click", "DoubleClick", "Enter", "Leave", "KeyDown", "KeyUp", "MouseClick", "MouseDown", "MouseUp", "MouseMove", "Resize", "Paint"] {
        add(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_fits_the_type() {
        let names = |t: &str, root: bool| properties_for(t, root).iter().map(|p| p.name).collect::<Vec<_>>();
        let button = names("System.Windows.Forms.Button", false);
        assert!(button.contains(&"Text") && button.contains(&"Location") && !button.contains(&"Checked") && !button.contains(&"ClientSize"));
        assert_eq!(property("Button", false, "TextAlign").unwrap().default, Some("MiddleCenter"));
        let timer = names("Timer", false);
        assert_eq!(timer, ["Enabled", "Interval"]);
        // Items have their own properties, none of a control's layout.
        let menu_item = names("System.Windows.Forms.ToolStripMenuItem", false);
        assert!(menu_item.contains(&"Text") && menu_item.contains(&"Checked") && menu_item.contains(&"ShortcutKeys"), "{menu_item:?}");
        assert!(!menu_item.contains(&"Location") && !menu_item.contains(&"Dock") && !menu_item.contains(&"TabIndex"));
        let column = names("DataGridViewTextBoxColumn", false);
        assert!(column.contains(&"HeaderText") && column.contains(&"Width") && column.contains(&"ReadOnly") && !column.contains(&"Text"), "{column:?}");
        assert_eq!(property("DataGridViewCheckBoxColumn", false, "Width").unwrap().default, Some("100"));
        assert_eq!(events_for("ToolStripMenuItem", false), ["Click", "CheckedChanged", "DropDownOpening"]);
        assert!(events_for("DataGridViewTextBoxColumn", false).is_empty());
        assert_eq!(item_collection("System.Windows.Forms.DataGridView"), Some(("Columns", "DataGridViewColumn")));
        assert_eq!(default_item_type("StatusStrip"), Some("ToolStripStatusLabel"));
        let form = names("Form", true);
        assert!(form.contains(&"ClientSize") && form.contains(&"Text") && !form.contains(&"Location"));
        assert!(names("UserControl", true).contains(&"Size"));
        assert_eq!(events_for("CheckBox", false)[0], "CheckedChanged");
        assert_eq!(event_info("KeyDown").args, "KeyEventArgs");
        assert!(is_component("System.Windows.Forms.Timer") && !is_component("Button") && is_component("MyCustomDialog"));
    }
}
