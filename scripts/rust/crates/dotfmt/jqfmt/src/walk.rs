use std::path::Path;

pub use dotfmt_core::walk::Gathered;
use dotfmt_core::walk::{self, Explicit, Symlinks};

use crate::dialect::Dialect;

pub fn gather(target: &Path) -> Result<Gathered, String> {
    walk::gather(target, Symlinks::Report, Explicit::Any, is_json)
}

fn is_json(path: &Path) -> bool {
    Dialect::for_path(path).is_some()
}
