#![forbid(unsafe_code)]

mod css;
mod curly;
mod dash;
mod docstring;
mod edit;
mod glyphs;
mod hash;
mod js;
mod keep;
pub mod lang;
mod luadoc;
pub mod purge;
mod scan;

pub use edit::{Change, Edited};
