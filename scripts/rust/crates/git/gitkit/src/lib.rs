//! Shared git for the tools in `crates/git`.
//!
//! Both tools start from the same picture of a working tree: what changed,
//! what throwing it away would cost, and where the repository is. gitoxide
//! draws that picture in-process — one parallel walk in place of a `git
//! status`, a `git diff` and a `git diff --no-index` per untracked file — and
//! writes the result back itself. What is left for the `git` binary is the
//! work that is genuinely its own: commits and pushes, where hooks, signing
//! and credentials live.
//!
//! The pieces are a [`Repo`] to open, a [`Survey`] of what changed, a [`View`]
//! to show it, and a discard to apply it.

mod discard;
mod measure;
mod render;
mod repo;
mod survey;

pub use render::View;
pub use repo::{Repo, git};
pub use survey::{Counts, Entry, Fate, Kind, Survey};

/// Where a repository-relative path is on this filesystem.
pub(crate) fn on_disk(root: &std::path::Path, path: &gix::bstr::BStr) -> std::path::PathBuf {
    root.join(gix::path::from_bstr(path))
}

/// A failure, carried as the line it will be printed as.
///
/// gitoxide's errors already read as sentences and chain their causes, so
/// nothing here needs to restate them; the tools print what they get.
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type Result<T> = std::result::Result<T, Error>;
