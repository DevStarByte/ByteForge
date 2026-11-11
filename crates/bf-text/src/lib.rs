//! Text engine for ByteForge: a rope-backed buffer with undo, and the editing model (carets,
//! selections and all editing commands). UI-independent and fully unit-tested.

mod buffer;
mod editor;
pub mod matcher;
mod undo;

pub use buffer::{Buffer, Change, LineEnding, Point};
pub use editor::{CharClass, Cursor, EditorState, char_class, logical_col, visual_col};
pub use matcher::{Matcher, Query};
pub use undo::UndoStack;
