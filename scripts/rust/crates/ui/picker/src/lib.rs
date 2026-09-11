#![forbid(unsafe_code)]

pub mod cascade;
pub mod fzf;
mod picker;
mod state;
pub use cascade::{Column, Pick, cascade, cascade_frame, cascade_in, cascade_outcome, choose};
pub use picker::Picker;
pub use state::{Item, Mode, SelectionState};
pub use ui_widgets::{MatchMode, Navigation, SearchIndex, SearchText};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<K> {
    Selected(Vec<K>),
    Cancelled,
    Interrupted,
    Unavailable,
}
