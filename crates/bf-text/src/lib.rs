//! Text engine for ByteForge: a rope-backed buffer with undo, and the editing model (carets,
//! selections and all editing commands). UI-independent and fully unit-tested.

mod buffer;
mod editor;
mod undo;

pub use buffer::{Buffer, Change, LineEnding, Point};
pub use editor::{CharClass, EditorState, char_class, logical_col, visual_col};
pub use undo::UndoStack;
