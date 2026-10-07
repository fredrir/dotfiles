use std::path::Path;

pub use dotfmt_core::walk::Gathered;
use dotfmt_core::walk::{self, Explicit, Symlinks};

pub fn gather(target: &Path) -> Result<Gathered, String> {
    walk::gather(target, Symlinks::Drop, Explicit::Regular, is_lua)
}

fn is_lua(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["lua", "luau"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}
