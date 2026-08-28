//! Where a target sits: relative to its git repository, or to the home
//! directory when it is outside one, or absolute when it is outside both.
//!
//! The repository root is found by walking up for a `.git` entry instead of
//! asking `git rev-parse --show-toplevel`. This runs inside prompts and
//! loops, where a process spawn is most of the cost, and the two answers
//! agree everywhere except inside `.git` itself: git declines to answer
//! there, while this prints `/.git/...`.
//!
//! Targets need not exist. The part that does is resolved through symlinks
//! and the rest is appended, so a path that is about to be created still
//! describes itself the way it will read once it is there.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use workstation::Completions;

const PROGRAM: &str = "path";

#[derive(Parser)]
#[command(
    version,
    about = "Print the repository-relative or home-relative path of a target"
)]
struct Cli {
    /// File or directory to describe
    #[arg(value_hint = ValueHint::AnyPath, default_value = ".")]
    target: PathBuf,

    /// Print the full path instead
    #[arg(short = 'f', long = "full")]
    full: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    let resolved = real_path(&cli.target);
    if cli.full {
        println!("{}", resolved.display());
        return ExitCode::SUCCESS;
    }
    let root = repository_root(&resolved);
    let home = std::env::var_os("HOME").map(|home| real_path(Path::new(&home)));
    println!("{}", describe(&resolved, root.as_deref(), home.as_deref()));
    ExitCode::SUCCESS
}

/// A repository root reads as `/`, so a path inside one looks like the same
/// path on another checkout of it; a home directory reads as `~`.
fn describe(resolved: &Path, root: Option<&Path>, home: Option<&Path>) -> String {
    if let Some(inside) = root.and_then(|root| relative(resolved, root)) {
        return format!("/{inside}");
    }
    match home.and_then(|home| relative(resolved, home)) {
        Some(inside) if inside.is_empty() => "~".to_string(),
        Some(inside) => format!("~/{inside}"),
        None => resolved.display().to_string(),
    }
}

/// Empty when the target is the base itself, `None` when it is elsewhere.
/// Whole components have to match, so `/repository` is not inside `/repo`.
fn relative(target: &Path, base: &Path) -> Option<String> {
    let inside = target.strip_prefix(base).ok()?;
    Some(inside.to_string_lossy().into_owned())
}

/// The nearest ancestor holding a `.git` entry: a directory in a plain
/// clone, a file in a worktree or a submodule.
fn repository_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// `realpath` for a target that need not exist: the longest existing prefix
/// is resolved through symlinks, and whatever is left is appended with `.`
/// and `..` folded in — a part of the path that does not exist has no
/// symlinks left to honour.
fn real_path(target: &Path) -> PathBuf {
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(target)
    };
    let parts: Vec<Component> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let head: PathBuf = parts[..split].iter().collect();
        if let Ok(resolved) = fs::canonicalize(&head) {
            return extend(resolved, &parts[split..]);
        }
    }
    extend(PathBuf::new(), &parts)
}

fn extend(mut base: PathBuf, rest: &[Component]) -> PathBuf {
    for part in rest {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                base.pop();
            }
            other => base.push(other),
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::create_dir_all(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/file.txt"), "").unwrap();
        root
    }

    fn described(target: &str, root: Option<&str>, home: Option<&str>) -> String {
        describe(Path::new(target), root.map(Path::new), home.map(Path::new))
    }

    #[test]
    fn inside_a_repository_the_root_is_the_top() {
        assert_eq!(described("/w/repo", Some("/w/repo"), None), "/");
        assert_eq!(
            described("/w/repo/sub/file", Some("/w/repo"), None),
            "/sub/file"
        );
    }

    #[test]
    fn a_repository_wins_over_the_home_directory() {
        assert_eq!(
            described("/home/u/repo/src", Some("/home/u/repo"), Some("/home/u")),
            "/src"
        );
    }

    #[test]
    fn outside_a_repository_the_home_directory_is_a_tilde() {
        assert_eq!(described("/home/u", None, Some("/home/u")), "~");
        assert_eq!(described("/home/u/docs", None, Some("/home/u")), "~/docs");
    }

    #[test]
    fn outside_both_the_path_is_left_alone() {
        assert_eq!(described("/usr/share", None, Some("/home/u")), "/usr/share");
        assert_eq!(described("/usr/share", None, None), "/usr/share");
    }

    #[test]
    fn a_shared_prefix_is_not_a_shared_directory() {
        assert_eq!(
            described("/w/repository/src", Some("/w/repo"), None),
            "/w/repository/src"
        );
        assert_eq!(
            described("/home/user2/x", None, Some("/home/user")),
            "/home/user2/x"
        );
    }

    #[test]
    fn the_root_is_the_nearest_ancestor_holding_git() {
        let root = repo();
        let real = fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            repository_root(&real.join("sub/file.txt")),
            Some(real.clone())
        );
        assert_eq!(repository_root(&real.join("does/not/exist")), Some(real));
    }

    #[test]
    fn a_git_file_marks_a_worktree_root() {
        let root = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(root.path()).unwrap();
        fs::write(real.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(repository_root(&real.join("sub")), Some(real));
    }

    #[test]
    fn a_directory_without_git_has_no_root() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        assert!(repository_root(&nested).is_none_or(|found| !found.starts_with(root.path())));
    }

    #[test]
    fn existing_targets_resolve_the_way_canonicalize_does() {
        let root = repo();
        let file = root.path().join("sub/file.txt");
        assert_eq!(real_path(&file), fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn missing_targets_keep_the_part_that_is_missing() {
        let root = repo();
        let real = fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            real_path(&root.path().join("missing/deep.txt")),
            real.join("missing/deep.txt")
        );
    }

    #[test]
    fn dots_fold_away_even_where_nothing_exists() {
        let root = repo();
        let real = fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            real_path(&root.path().join("missing/../other")),
            real.join("other")
        );
        assert_eq!(
            real_path(&root.path().join("./sub/../sub")),
            real.join("sub")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_the_existing_part_are_followed() {
        let root = repo();
        let real = fs::canonicalize(root.path()).unwrap();
        std::os::unix::fs::symlink(real.join("sub"), real.join("link")).unwrap();
        assert_eq!(
            real_path(&real.join("link/new.txt")),
            real.join("sub/new.txt")
        );
    }
}
