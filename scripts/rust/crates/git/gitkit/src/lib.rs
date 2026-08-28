
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

pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type Result<T> = std::result::Result<T, Error>;
