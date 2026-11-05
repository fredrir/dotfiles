#![forbid(unsafe_code)]

mod css;
mod curly;
mod docstring;
mod edit;
pub mod engine;
mod glyphs;
mod hash;
mod js;
mod keep;
pub mod lang;
mod luadoc;
pub mod purge;
mod scan;
pub mod syntax;

pub use edit::{Change, Edited};

#[doc(hidden)]
pub mod bridge {
    pub use crate::curly::scan as curly_scan;
    pub use crate::engine::generic::scan as generic_scan;
    pub use crate::scan::{Bail, Comment, Logical, Out, Text};
}
