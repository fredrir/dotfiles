#![forbid(unsafe_code)]

mod explorer;
mod local;
mod model;
mod render;
mod source;
mod state;
mod terminal;
mod view;

pub use explorer::{
    Explorer, ExplorerConfig, ExplorerError, Layout, Prefetch, SystemExplorerError,
    SystemExplorerResult, TerminalExplorerError, TerminalExplorerResult,
};
pub use local::{LocalError, LocalSource};
pub use model::{
    AcceptTarget, Directory, DirectoryStatus, Entry, EntryKind, InputKind, Outcome, Selection,
    SelectionPolicy,
};
pub use source::FileSource;
pub use terminal::{Key, ScriptedTerminal, Size, SystemTerminal, Terminal};
pub use view::{DefaultView, ExplorerView, Line, Role, Span, ViewContext};
