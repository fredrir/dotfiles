mod discard;
mod measure;
mod render;
mod repo;
mod survey;

pub use render::View;
pub use repo::{Repo, git};
pub use survey::{Counts, Entry, Fate, Kind, Survey};

pub(crate) fn on_disk(root: &std::path::Path, path: &gix::bstr::BStr) -> std::path::PathBuf {
    root.join(gix::path::from_bstr(path))
}

pub(crate) fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
