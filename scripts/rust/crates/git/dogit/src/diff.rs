use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const OUTPUT_LIMIT: u64 = 32 * 1024 * 1024;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffSide {
    Worktree,
    Index,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub id: usize,
    pub kind: LineKind,
    /// Exact bytes after the patch prefix, including the line terminator.
    pub content: Vec<u8>,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub no_newline: bool,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub header: String,
    pub path: String,
    pub file_path: PathBuf,
    pub lines: Vec<DiffLine>,
    old_start: usize,
    new_start: usize,
}

#[derive(Clone, Debug)]
pub enum Selection {
    Hunk(usize),
    Lines(Vec<usize>),
}

#[derive(Clone, Debug)]
struct Snapshot {
    index_path: PathBuf,
    index: Option<Vec<u8>>,
    worktree: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct RevisionComparison {
    old: Option<String>,
    new: String,
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    root: &'a Path,
    cancel: Option<&'a AtomicBool>,
}

impl Reader<'_> {
    fn git<I, S>(self, args: I) -> Result<Vec<u8>, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = git_command(self.root);
        command.args(args);
        run(command, None, false, self.cancel)
    }
}

#[derive(Clone, Debug)]
pub struct ExactDiff {
    pub path: PathBuf,
    pub side: DiffSide,
    pub hunks: Vec<Hunk>,
    pub raw: Vec<u8>,
    pub partial_reason: Option<String>,
    root: PathBuf,
    snapshot: Option<Snapshot>,
    base: Option<Vec<u8>>,
    target: Option<Vec<u8>>,
    old_exists: bool,
    new_exists: bool,
    file_mode: String,
    comparison: Option<RevisionComparison>,
}

impl ExactDiff {
    pub fn load(root: &Path, path: &Path, side: DiffSide) -> Result<Self, String> {
        Self::load_cancellable(root, path, side, None)
    }

    pub fn load_cancellable(
        root: &Path,
        path: &Path,
        side: DiffSide,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, String> {
        validate_path(path)?;
        let reader = Reader { root, cancel };
        let index_path = index_path(reader)?;
        let index = read_optional(&index_path)?;
        let index_entry = reader.git([
            OsStr::new("ls-files"),
            OsStr::new("--stage"),
            OsStr::new("-z"),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        let tracked = !index_entry.is_empty();
        let worktree = if side == DiffSide::Worktree {
            read_worktree(&root.join(path))?
        } else {
            None
        };
        let mut args = diff_args();
        if side == DiffSide::Index {
            args.push("--cached".into());
        }
        args.push("--".into());
        args.push(path.as_os_str().to_owned());
        let raw = if side == DiffSide::Worktree && !tracked {
            untracked_diff(reader, path)?
        } else {
            reader.git(args)?
        };
        let mut document = Self::parse(root, path, side, raw)?;
        if index_entry.split(|byte| *byte == 0).any(|entry| {
            entry
                .split(|byte| *byte == b' ')
                .nth(2)
                .is_some_and(|stage| !stage.starts_with(b"0\t"))
        }) {
            document.partial_reason =
                Some("Resolve the conflict before staging individual lines".into());
        }
        let base_revision = if side == DiffSide::Index { "HEAD" } else { "" };
        if document.partial_reason.is_none() && !document.raw.is_empty() {
            let base = if document.old_exists && (side == DiffSide::Index || tracked) {
                read_blob(reader, base_revision, path)?
            } else {
                Vec::new()
            };
            document.target = Some(reconstruct(&base, &document.hunks)?);
            document.base = Some(base);
        }
        if read_optional(&index_path)? != index
            || (side == DiffSide::Worktree && read_worktree(&root.join(path))? != worktree)
        {
            return Err("Repository changed while reading the diff; refresh and try again".into());
        }
        document.snapshot = Some(Snapshot {
            index_path,
            index,
            worktree,
        });
        Ok(document)
    }

    pub fn between(
        root: &Path,
        old_revision: &str,
        new_revision: &str,
        path: Option<&Path>,
    ) -> Result<Self, String> {
        Self::between_cancellable(root, old_revision, new_revision, path, None)
    }

    /// An exact aggregate preview of the next commit. Partial index mutations
    /// require an individual file and its corresponding index snapshot.
    pub fn staged(root: &Path) -> Result<Self, String> {
        Self::staged_cancellable(root, None)
    }

    pub fn staged_cancellable(root: &Path, cancel: Option<&AtomicBool>) -> Result<Self, String> {
        let reader = Reader { root, cancel };
        let mut args = diff_args();
        args.extend([OsString::from("--cached"), OsString::from("--")]);
        let mut document = Self::parse(root, Path::new(""), DiffSide::Index, reader.git(args)?)?;
        document.partial_reason = Some("Select an individual file before unstaging lines".into());
        Ok(document)
    }

    /// Inspect a directory across either side of the index. Aggregates never
    /// carry the single-file snapshot required for partial index mutations.
    pub fn directory_cancellable(
        root: &Path,
        path: &Path,
        side: DiffSide,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, String> {
        if !path.as_os_str().is_empty() {
            validate_path(path)?;
        }
        let pathspec = if path.as_os_str().is_empty() {
            OsStr::new(".")
        } else {
            path.as_os_str()
        };
        let reader = Reader { root, cancel };
        let mut args = diff_args();
        if side == DiffSide::Index {
            args.push("--cached".into());
        }
        args.extend([OsString::from("--"), pathspec.to_owned()]);
        let mut raw = reader.git(args)?;
        if side == DiffSide::Worktree {
            let untracked = reader.git([
                OsStr::new("ls-files"),
                OsStr::new("--others"),
                OsStr::new("--exclude-standard"),
                OsStr::new("-z"),
                OsStr::new("--"),
                pathspec,
            ])?;
            let started = std::time::Instant::now();
            for bytes in untracked
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                if started.elapsed() > PROCESS_TIMEOUT {
                    return Err("Directory preview took too long; select an individual file".into());
                }
                let file = path_from_bytes(bytes)?;
                validate_path(&file)?;
                if file.starts_with(path) {
                    let patch = untracked_diff(reader, &file)?;
                    if raw.len().saturating_add(patch.len()) > OUTPUT_LIMIT as usize {
                        return Err(
                            "Directory preview exceeds 32 MiB; select an individual file".into(),
                        );
                    }
                    raw.extend(patch);
                }
            }
        }
        let mut document = Self::parse(root, Path::new(""), side, raw)?;
        if !document.raw.is_empty() {
            document.partial_reason =
                Some("Select an individual file to stage or unstage lines".into());
        }
        Ok(document)
    }

    pub fn between_cancellable(
        root: &Path,
        old_revision: &str,
        new_revision: &str,
        path: Option<&Path>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, String> {
        let reader = Reader { root, cancel };
        let old = resolve_revision(reader, old_revision)?;
        let new = resolve_revision(reader, new_revision)?;
        let mut args = diff_args();
        args.extend([old.clone().into(), new.clone().into(), "--".into()]);
        if let Some(path) = path {
            validate_path(path)?;
            args.push(path.as_os_str().to_owned());
        }
        let mut document = Self::parse(
            root,
            path.unwrap_or(Path::new("")),
            DiffSide::Index,
            reader.git(args)?,
        )?;
        document.partial_reason = Some("History comparisons are read-only".into());
        document.comparison = Some(RevisionComparison {
            old: Some(old),
            new,
        });
        Ok(document)
    }

    pub fn commit(root: &Path, oid: &str) -> Result<Self, String> {
        Self::commit_cancellable(root, oid, None, None)
    }

    pub fn commit_file(root: &Path, oid: &str, path: &Path) -> Result<Self, String> {
        Self::commit_cancellable(root, oid, Some(path), None)
    }

    pub fn commit_cancellable(
        root: &Path,
        oid: &str,
        path: Option<&Path>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, String> {
        let reader = Reader { root, cancel };
        let oid = resolve_revision(reader, oid)?;
        let parents = reader.git(["rev-list", "--parents", "--max-count=1", &oid, "--"])?;
        let old = String::from_utf8_lossy(&parents)
            .split_whitespace()
            .nth(1)
            .map(str::to_owned);
        let mut args = [
            "show",
            "--format=",
            "--first-parent",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--binary",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &oid,
            "--",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        if let Some(path) = path {
            validate_path(path)?;
            args.push(path.as_os_str().to_owned());
        }
        let mut document = Self::parse(
            root,
            path.unwrap_or(Path::new("")),
            DiffSide::Index,
            reader.git(args)?,
        )?;
        document.partial_reason = Some("History comparisons are read-only".into());
        document.comparison = Some(RevisionComparison { old, new: oid });
        Ok(document)
    }

    fn parse(root: &Path, path: &Path, side: DiffSide, raw: Vec<u8>) -> Result<Self, String> {
        let hunks = parse_hunks(&raw)?;
        let headers = raw
            .split(|byte| *byte == b'\n')
            .filter(|line| !matches!(line.first(), Some(b'+' | b'-' | b' ')))
            .collect::<Vec<_>>();
        let contains = |prefix: &[u8]| headers.iter().any(|line| line.starts_with(prefix));
        let partial_reason = if contains(b"GIT binary patch") || contains(b"Binary files ") {
            Some("Binary files must be staged or unstaged as a whole file".into())
        } else if contains(b"rename from ") || contains(b"copy from ") {
            Some("Renames and copies must be staged or unstaged as a whole file".into())
        } else if contains(b"old mode ") || contains(b"new mode ") {
            Some("File mode changes must be staged or unstaged as a whole file".into())
        } else if headers
            .iter()
            .any(|line| line.ends_with(b"120000") || line.ends_with(b"160000"))
        {
            Some("Symbolic links and submodules must be staged or unstaged as a whole file".into())
        } else if hunks.is_empty() && !raw.is_empty() {
            Some("This change has no selectable text; use the whole-file action".into())
        } else {
            None
        };
        let old_exists = !contains(b"new file mode ");
        let new_exists = !contains(b"deleted file mode ");
        let file_mode = headers
            .iter()
            .find_map(|line| {
                line.strip_prefix(b"new file mode ")
                    .or_else(|| line.strip_prefix(b"deleted file mode "))
            })
            .map(|mode| String::from_utf8_lossy(mode).into_owned())
            .unwrap_or_else(|| "100644".into());
        Ok(Self {
            path: path.to_owned(),
            side,
            hunks,
            raw,
            partial_reason,
            root: root.to_owned(),
            snapshot: None,
            base: None,
            target: None,
            old_exists,
            new_exists,
            file_mode,
            comparison: None,
        })
    }

    /// Apply to a temporary index, then publish it under Git's index lock.
    /// The working tree is never modified by either staging or unstaging.
    pub fn apply(&self, selection: &Selection) -> Result<(), String> {
        if let Some(reason) = &self.partial_reason {
            return Err(reason.clone());
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or("History comparisons are read-only")?;
        let patch = self.selected_patch(selection)?;
        let lock_path = append_suffix(&snapshot.index_path, ".lock");
        let mut lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "Another Git operation is updating the index; try again when it finishes".into()
                } else {
                    format!("Cannot lock the index: {error}")
                }
            })?;
        let guard = IndexLock(Some(lock_path));
        if read_optional(&snapshot.index_path)? != snapshot.index {
            return Err("The index changed; refresh the diff before staging or unstaging".into());
        }
        if snapshot.index.is_some() {
            lock.set_permissions(
                fs::metadata(&snapshot.index_path)
                    .map_err(|error| error.to_string())?
                    .permissions(),
            )
            .map_err(|error| format!("Cannot preserve index permissions: {error}"))?;
        }
        if self.side == DiffSide::Worktree
            && read_worktree(&self.root.join(&self.path))? != snapshot.worktree
        {
            return Err("The file changed; refresh the diff before staging".into());
        }
        let fresh = Self::load(&self.root, &self.path, self.side)?;
        if fresh.raw != self.raw {
            return Err("The diff changed; refresh before staging or unstaging".into());
        }
        // A split index locates sharedindex.* beside its own index file. Keep
        // this copy in that same directory, then remove its shared dependency.
        let temp_index = tempfile::Builder::new()
            .prefix("dogit-index-")
            .tempfile_in(
                snapshot
                    .index_path
                    .parent()
                    .ok_or("Index has no parent directory")?,
            )
            .map_err(|error| error.to_string())?;
        if let Some(bytes) = &snapshot.index {
            fs::write(temp_index.path(), bytes).map_err(|error| error.to_string())?;
            let mut command = git_command(&self.root);
            command
                .env("GIT_INDEX_FILE", temp_index.path())
                .args(["update-index", "--no-split-index"]);
            run(command, None, false, None)?;
        } else {
            fs::remove_file(temp_index.path()).map_err(|error| error.to_string())?;
        }
        let mut command = git_command(&self.root);
        command.env("GIT_INDEX_FILE", temp_index.path()).args([
            "-c",
            "core.splitIndex=false",
            "apply",
            "--cached",
            "--whitespace=nowarn",
            "--unidiff-zero",
            "-",
        ]);
        run(command, Some(patch), false, None)?;
        let updated = fs::read(temp_index.path()).map_err(|error| error.to_string())?;
        lock.write_all(&updated)
            .map_err(|error| format!("Cannot write the index: {error}"))?;
        lock.sync_all()
            .map_err(|error| format!("Cannot save the index: {error}"))?;
        drop(lock);
        guard.publish(&snapshot.index_path)?;
        Ok(())
    }

    pub fn selected_patch(&self, selection: &Selection) -> Result<Vec<u8>, String> {
        if let Some(reason) = &self.partial_reason {
            return Err(reason.clone());
        }
        let selected: BTreeSet<usize> = match selection {
            Selection::Hunk(index) => self
                .hunks
                .get(*index)
                .ok_or("The selected hunk no longer exists")?
                .lines
                .iter()
                .filter(|line| line.kind != LineKind::Context)
                .map(|line| line.id)
                .collect(),
            Selection::Lines(ids) => ids.iter().copied().collect(),
        };
        let changed: BTreeSet<usize> = self
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind != LineKind::Context)
            .map(|line| line.id)
            .collect();
        if selected.is_empty() || !selected.is_subset(&changed) {
            return Err("Select changed lines or a hunk first".into());
        }
        let reverse = self.side == DiffSide::Index;
        let (before_exists, after_exists) = if reverse {
            (self.new_exists, self.old_exists)
        } else {
            (self.old_exists, self.new_exists)
        };
        let remove = !after_exists && selected == changed;
        let mut patch = Vec::new();
        let old_path = quote_path(b"a/", &self.path);
        let new_path = quote_path(b"b/", &self.path);
        patch.extend_from_slice(format!("diff --git {old_path} {new_path}\n").as_bytes());
        if !before_exists {
            patch.extend_from_slice(format!("new file mode {}\n", self.file_mode).as_bytes());
        }
        if remove {
            patch.extend_from_slice(format!("deleted file mode {}\n", self.file_mode).as_bytes());
        }
        patch.extend_from_slice(
            format!(
                "--- {}\n+++ {}\n",
                if before_exists {
                    &old_path
                } else {
                    "/dev/null"
                },
                if remove { "/dev/null" } else { &new_path }
            )
            .as_bytes(),
        );
        let mut offset: isize = 0;
        let mut unterminated_output = false;
        for hunk in &self.hunks {
            if !hunk.lines.iter().any(|line| selected.contains(&line.id)) {
                continue;
            }
            let mut body = Vec::new();
            let mut old_count = 0usize;
            let mut new_count = 0usize;
            for line in &hunk.lines {
                let kind = if reverse {
                    match line.kind {
                        LineKind::Addition => LineKind::Deletion,
                        LineKind::Deletion => LineKind::Addition,
                        other => other,
                    }
                } else {
                    line.kind
                };
                let prefix = match (kind, selected.contains(&line.id)) {
                    (LineKind::Addition, false) => continue,
                    (LineKind::Deletion, false) | (LineKind::Context, _) => b' ',
                    (LineKind::Addition, true) => b'+',
                    (LineKind::Deletion, true) => b'-',
                };
                if prefix != b'-' {
                    if unterminated_output {
                        return Err("Include the final-line replacement or select the whole hunk to preserve line endings".into());
                    }
                    unterminated_output = line.no_newline;
                }
                old_count += usize::from(prefix != b'+');
                new_count += usize::from(prefix != b'-');
                body.push(prefix);
                body.extend_from_slice(&line.content);
                if line.no_newline {
                    body.extend_from_slice(b"\n\\ No newline at end of file\n");
                }
            }
            let old_start = if reverse {
                hunk.new_start
            } else {
                hunk.old_start
            };
            let insertion_start = if old_count == 0 {
                old_start + 1
            } else {
                old_start
            };
            let new_start =
                (insertion_start as isize + offset - isize::from(new_count == 0)).max(0) as usize;
            patch.extend_from_slice(
                format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes(),
            );
            patch.extend_from_slice(&body);
            offset += new_count as isize - old_count as isize;
        }
        Ok(patch)
    }

    /// Structural presentation is inspection only; staging always uses exact hunks.
    /// Missing Difftastic is an ordinary fallback, represented by `None`.
    pub fn structural(&self, width: usize) -> Result<Option<String>, String> {
        self.structural_cancellable(width, None)
    }

    pub fn structural_cancellable(
        &self,
        width: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<Option<String>, String> {
        self.structural_using(width, cancel, OsStr::new("difft"))
    }

    fn structural_using(
        &self,
        width: usize,
        cancel: Option<&AtomicBool>,
        program: &OsStr,
    ) -> Result<Option<String>, String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let before_dir = temp.path().join("before");
        let after_dir = temp.path().join("after");
        fs::create_dir(&before_dir).map_err(|error| error.to_string())?;
        fs::create_dir(&after_dir).map_err(|error| error.to_string())?;
        let paths;
        if let Some(comparison) = &self.comparison {
            let reader = Reader {
                root: &self.root,
                cancel,
            };
            paths = if self.path.as_os_str().is_empty() {
                comparison.paths(reader)?
            } else {
                vec![self.path.clone()]
            };
            let mut captured = 0usize;
            for path in &paths {
                validate_path(path)?;
                let base = comparison
                    .old
                    .as_ref()
                    .map(|old| read_revision_file(reader, old, path))
                    .transpose()?
                    .flatten();
                let target = read_revision_file(reader, &comparison.new, path)?;
                captured = captured
                    .saturating_add(base.as_ref().map_or(0, Vec::len))
                    .saturating_add(target.as_ref().map_or(0, Vec::len));
                if captured > OUTPUT_LIMIT as usize {
                    return Err(
                        "Structural comparison exceeds 32 MiB; select an individual file".into(),
                    );
                }
                if let Some(base) = base {
                    write_comparison_file(&before_dir, path, &base)?;
                }
                if let Some(target) = target {
                    write_comparison_file(&after_dir, path, &target)?;
                }
            }
        } else if let (Some(base), Some(target)) = (&self.base, &self.target) {
            paths = vec![self.path.clone()];
            write_comparison_file(&before_dir, &self.path, base)?;
            write_comparison_file(&after_dir, &self.path, target)?;
        } else {
            return Ok(None);
        }
        let command = || {
            let mut command = Command::new(program);
            command
                .current_dir(temp.path())
                .env_remove("DFT_CHECK_ONLY")
                .env_remove("DFT_IGNORE_COMMENTS")
                .args([
                    "--color=always",
                    "--background=dark",
                    "--display=side-by-side",
                    "--context=3",
                    "--sort-paths",
                    "--strip-cr=off",
                    "--syntax-highlight=on",
                ])
                .arg(format!("--width={}", width.max(20)));
            command
        };
        let mut commands = Vec::new();
        if paths
            .iter()
            .any(|path| before_dir.join(path).is_dir() || after_dir.join(path).is_dir())
        {
            // Difftastic's directory walker cannot compare a file replaced by
            // a directory. Its Git external-diff interface handles each leaf.
            for path in paths {
                let before = before_dir.join(&path);
                let after = after_dir.join(&path);
                let mut command = command();
                command
                    .arg(&path)
                    .arg(if before.is_file() {
                        &before
                    } else {
                        Path::new("/dev/null")
                    })
                    .args(["0", "100644"])
                    .arg(if after.is_file() {
                        &after
                    } else {
                        Path::new("/dev/null")
                    })
                    .args(["0", "100644"]);
                commands.push(command);
            }
        } else {
            let mut command = command();
            command.args(["before", "after"]);
            commands.push(command);
        }
        let mut result = Vec::new();
        for command in commands {
            match run(command, None, true, cancel) {
                Ok(bytes) => result.extend_from_slice(&bytes),
                Err(error) if error.starts_with("Executable unavailable:") => return Ok(None),
                Err(error) => return Err(error),
            }
            if result.len() > OUTPUT_LIMIT as usize {
                return Err("Structural output exceeds 32 MiB; select an individual file".into());
            }
        }
        Ok(Some(String::from_utf8_lossy(&result).into_owned()))
    }
}

impl RevisionComparison {
    fn paths(&self, reader: Reader<'_>) -> Result<Vec<PathBuf>, String> {
        let bytes = if let Some(old) = &self.old {
            reader.git([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--name-only",
                "-z",
                old,
                &self.new,
                "--",
            ])?
        } else {
            reader.git([
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--no-renames",
                "--name-only",
                "-z",
                "-r",
                &self.new,
                "--",
            ])?
        };
        bytes
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(path_from_bytes)
            .collect()
    }
}

fn write_comparison_file(directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    validate_path(path)?;
    let destination = directory.join(path);
    fs::create_dir_all(destination.parent().ok_or("File has no parent")?)
        .map_err(|error| error.to_string())?;
    fs::write(destination, bytes).map_err(|error| error.to_string())
}

struct IndexLock(Option<PathBuf>);
impl IndexLock {
    fn publish(mut self, destination: &Path) -> Result<(), String> {
        let path = self.0.as_ref().ok_or("Index lock was already released")?;
        fs::rename(path, destination)
            .map_err(|error| format!("Cannot publish the index: {error}"))?;
        self.0 = None;
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn patch_file_path(raw: &[u8]) -> Option<PathBuf> {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    if raw == b"/dev/null" {
        return None;
    }
    let mut bytes = Vec::new();
    if raw.first() == Some(&b'"') {
        let mut input = raw[1..].iter().copied();
        while let Some(byte) = input.next() {
            match byte {
                b'"' => break,
                b'\\' => {
                    let escaped = input.next()?;
                    let decoded = match escaped {
                        b'a' => 7,
                        b'b' => 8,
                        b't' => b'\t',
                        b'n' => b'\n',
                        b'v' => 11,
                        b'f' => 12,
                        b'r' => b'\r',
                        b'\\' | b'"' => escaped,
                        b'0'..=b'3' => {
                            let second = input.next()?.checked_sub(b'0').filter(|b| *b < 8)?;
                            let third = input.next()?.checked_sub(b'0').filter(|b| *b < 8)?;
                            (escaped - b'0') * 64 + second * 8 + third
                        }
                        _ => return None,
                    };
                    bytes.push(decoded);
                }
                _ => bytes.push(byte),
            }
        }
    } else {
        bytes.extend(raw.split(|byte| *byte == b'\t').next()?);
    }
    let bytes = bytes
        .strip_prefix(b"a/")
        .or_else(|| bytes.strip_prefix(b"b/"))?
        .to_vec();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

fn parse_hunks(raw: &[u8]) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut path = String::new();
    let mut file_path = PathBuf::new();
    let mut old_line = 0;
    let mut new_line = 0;
    let mut id = 0;
    let mut remaining_old = 0;
    let mut remaining_new = 0;
    for line in raw.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"diff --git ") {
            path = String::from_utf8_lossy(line).trim_end().to_owned();
            file_path.clear();
        } else if remaining_old == 0
            && remaining_new == 0
            && (line.starts_with(b"--- ") || line.starts_with(b"+++ "))
        {
            if let Some(parsed) = patch_file_path(&line[4..]) {
                file_path = parsed;
            }
        } else if line.starts_with(b"@@ ") {
            let header = String::from_utf8_lossy(line).trim_end().to_owned();
            let mut ranges = header.split_whitespace().skip(1);
            let (old_start, old_count) =
                parse_range(ranges.next().ok_or("Missing old hunk range")?)?;
            let (new_start, new_count) =
                parse_range(ranges.next().ok_or("Missing new hunk range")?)?;
            old_line = old_start;
            new_line = new_start;
            remaining_old = old_count;
            remaining_new = new_count;
            hunks.push(Hunk {
                header,
                path: path.clone(),
                file_path: file_path.clone(),
                lines: Vec::new(),
                old_start,
                new_start,
            });
        } else if line.starts_with(b"\\ No newline at end of file") {
            if let Some(previous) = hunks.last_mut().and_then(|hunk| hunk.lines.last_mut()) {
                previous.no_newline = true;
                if previous.content.last() == Some(&b'\n') {
                    previous.content.pop();
                }
            }
        } else if remaining_old > 0 || remaining_new > 0 {
            let kind = match line.first() {
                Some(b' ') => LineKind::Context,
                Some(b'+') => LineKind::Addition,
                Some(b'-') => LineKind::Deletion,
                _ => return Err("Unrecognized line in Git patch".into()),
            };
            let has_old = kind != LineKind::Addition;
            let has_new = kind != LineKind::Deletion;
            remaining_old = remaining_old
                .checked_sub(usize::from(has_old))
                .ok_or("Invalid old hunk count")?;
            remaining_new = remaining_new
                .checked_sub(usize::from(has_new))
                .ok_or("Invalid new hunk count")?;
            hunks
                .last_mut()
                .ok_or("Patch line without hunk")?
                .lines
                .push(DiffLine {
                    id,
                    kind,
                    content: line[1..].to_vec(),
                    old_line: has_old.then_some(old_line),
                    new_line: has_new.then_some(new_line),
                    no_newline: false,
                });
            id += 1;
            old_line += usize::from(has_old);
            new_line += usize::from(has_new);
        }
    }
    if remaining_old != 0 || remaining_new != 0 {
        return Err("Git returned an incomplete patch".into());
    }
    Ok(hunks)
}

fn parse_range(range: &str) -> Result<(usize, usize), String> {
    let mut parts = range.get(1..).ok_or("Invalid patch range")?.split(',');
    let start = parts
        .next()
        .unwrap_or_default()
        .parse()
        .map_err(|_| "Invalid patch start")?;
    let count = parts
        .next()
        .unwrap_or("1")
        .parse()
        .map_err(|_| "Invalid patch length")?;
    Ok((start, count))
}

fn reconstruct(base: &[u8], hunks: &[Hunk]) -> Result<Vec<u8>, String> {
    let lines = base
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let mut position = 0;
    let mut result = Vec::new();
    for hunk in hunks {
        let start = if hunk.lines.iter().any(|line| line.old_line.is_some()) {
            hunk.old_start.saturating_sub(1)
        } else {
            hunk.old_start
        };
        if start < position || start > lines.len() {
            return Err("Patch does not match the captured source".into());
        }
        for line in &lines[position..start] {
            result.extend_from_slice(line);
        }
        position = start;
        for line in &hunk.lines {
            if line.kind != LineKind::Addition {
                if lines.get(position).copied() != Some(line.content.as_slice()) {
                    return Err("Patch source differs from captured source".into());
                }
                position += 1;
            }
            if line.kind != LineKind::Deletion {
                result.extend_from_slice(&line.content);
            }
        }
    }
    for line in &lines[position..] {
        result.extend_from_slice(line);
    }
    Ok(result)
}

fn diff_args() -> Vec<OsString> {
    [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--binary",
        "--full-index",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--unified=3",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

fn untracked_diff(reader: Reader<'_>, path: &Path) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(reader.root.join(path))
        .is_ok_and(|metadata| metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Ok(Vec::new());
    }
    let mut command = git_command(reader.root);
    command
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--binary",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            "/dev/null",
        ])
        .arg(path);
    run(command, None, true, reader.cancel)
}

fn read_blob(reader: Reader<'_>, revision: &str, path: &Path) -> Result<Vec<u8>, String> {
    let mut object = OsString::from(revision);
    object.push(":");
    object.push(path);
    reader.git([OsString::from("cat-file"), OsString::from("blob"), object])
}

fn read_revision_file(
    reader: Reader<'_>,
    revision: &str,
    path: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let entry = reader.git([
        OsStr::new("ls-tree"),
        OsStr::new("--full-tree"),
        OsStr::new("-z"),
        OsStr::new(revision),
        OsStr::new("--"),
        path.as_os_str(),
    ])?;
    if entry.is_empty() {
        return Ok(None);
    }
    let header = entry
        .split(|byte| *byte == b'\t')
        .next()
        .ok_or("Malformed tree entry")?;
    let mut fields = header.split(|byte| *byte == b' ');
    let mode = fields.next();
    let kind = fields.next();
    let oid = fields.next().ok_or("Tree entry is missing its object ID")?;
    if mode == Some(b"160000".as_slice()) {
        return Ok(Some(
            format!("Subproject commit {}\n", String::from_utf8_lossy(oid)).into_bytes(),
        ));
    }
    // A path can be a blob on one side and a directory on the other. Its
    // descendant blobs are separate entries in the changed-path list.
    if kind == Some(b"tree".as_slice()) {
        return Ok(None);
    }
    if kind != Some(b"blob".as_slice()) {
        return Err("Select a file for structural comparison".into());
    }
    Ok(Some(read_blob(reader, revision, path)?))
}

fn resolve_revision(reader: Reader<'_>, revision: &str) -> Result<String, String> {
    let output = reader.git([
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{revision}^{{commit}}"),
    ])?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn index_path(reader: Reader<'_>) -> Result<PathBuf, String> {
    let bytes = reader.git(["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    path_from_bytes(bytes)
}

fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
}

fn validate_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("Diff paths must be relative paths inside the repository".into());
    }
    Ok(())
}

fn quote_path(prefix: &[u8], path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path.to_str().unwrap_or_default().as_bytes();
    let mut result = String::from("\"");
    for byte in prefix.iter().chain(bytes) {
        match byte {
            b'"' => result.push_str("\\\""),
            b'\\' => result.push_str("\\\\"),
            32..=126 => result.push(char::from(*byte)),
            _ => result.push_str(&format!("\\{byte:03o}")),
        }
    }
    result.push('"');
    result
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read {}: {error}", path.display())),
    }
}

fn read_worktree(path: &Path) -> Result<Option<Vec<u8>>, String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(Some(
            fs::read_link(path)
                .map_err(|error| error.to_string())?
                .into_os_string()
                .as_encoded_bytes()
                .to_vec(),
        ));
    }
    if path.is_dir() {
        return Ok(None);
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("File exceeds the 32 MiB preview limit; use whole-file staging".into());
    }
    Ok(Some(bytes))
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(["-c", "diff.suppressBlankEmpty=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("LC_ALL", "C");
    command
}

fn run(
    mut command: Command,
    input: Option<Vec<u8>>,
    allow_difference: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, String> {
    if let Some(input) = input {
        let mut file = tempfile::tempfile().map_err(|error| error.to_string())?;
        file.write_all(&input).map_err(|error| error.to_string())?;
        file.rewind().map_err(|error| error.to_string())?;
        command.stdin(Stdio::from(file));
    } else {
        command.stdin(Stdio::null());
    }
    let output = hostkit::process::output_cancellable(
        &mut command,
        hostkit::process::CaptureLimits {
            stdout: OUTPUT_LIMIT as usize,
            stderr: 128 * 1024,
        },
        PROCESS_TIMEOUT,
        &|| cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)),
    )
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("Executable unavailable: {error}"),
        std::io::ErrorKind::TimedOut => {
            "Git or structural diff took too long; try a smaller comparison".into()
        }
        _ => error.to_string(),
    })?;
    if output.stdout_truncated {
        return Err("Diff output exceeds 32 MiB; select a smaller comparison".into());
    }
    if output.status.success() || (allow_difference && output.status.code() == Some(1)) {
        Ok(output.stdout)
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            "Git or structural diff could not complete the operation".into()
        } else {
            message
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        command(root, &["init", "-q"]);
        command(root, &["config", "user.name", "Dogit test"]);
        command(root, &["config", "user.email", "dogit@example.invalid"]);
        command(root, &["config", "commit.gpgsign", "false"]);
        command(root, &["config", "core.hooksPath", "/dev/null"]);
        directory
    }

    fn command(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn executable(root: &Path, script: &str) -> PathBuf {
        let program = root.join("inspect-structural");
        fs::write(&program, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        program
    }

    #[test]
    fn structural_comparisons_capture_every_file_and_keep_syntax_extensions() {
        let directory = fixture();
        let root = directory.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/one.rs"), b"fn one() {}\n").unwrap();
        fs::write(root.join("removed.py"), b"old = True\n").unwrap();
        command(root, &["add", "."]);
        command(root, &["commit", "-qm", "before"]);
        fs::write(root.join("src/one.rs"), b"fn one() { two(); }\n").unwrap();
        fs::remove_file(root.join("removed.py")).unwrap();
        fs::write(root.join("added.js"), b"const added = true;\n").unwrap();
        command(root, &["add", "."]);
        command(root, &["commit", "-qm", "after"]);
        let program = executable(
            root,
            "cat before/src/one.rs after/src/one.rs before/removed.py after/added.js\ntest ! -s after/removed.py\ntest ! -s before/added.js",
        );
        let diff = ExactDiff::between(root, "HEAD~", "HEAD", None).unwrap();
        let text = diff
            .structural_using(110, None, program.as_os_str())
            .unwrap()
            .unwrap();
        assert_eq!(
            text,
            "fn one() {}\nfn one() { two(); }\nold = True\nconst added = true;\n"
        );
    }

    #[test]
    fn structural_root_commit_has_no_before_file() {
        let directory = fixture();
        let root = directory.path();
        fs::write(root.join("main.rs"), b"fn main() {}\n").unwrap();
        command(root, &["add", "."]);
        command(root, &["commit", "-qm", "root"]);
        let program = executable(root, "test ! -e before/main.rs\ncat after/main.rs");
        let diff = ExactDiff::commit_file(root, "HEAD", Path::new("main.rs")).unwrap();
        assert_eq!(
            diff.structural_using(80, None, program.as_os_str())
                .unwrap()
                .unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn structural_staging_source_excludes_worktree_changes_on_the_index_side() {
        let directory = fixture();
        let root = directory.path();
        fs::write(root.join("main.rs"), b"fn original() {}\n").unwrap();
        command(root, &["add", "."]);
        command(root, &["commit", "-qm", "root"]);
        fs::write(root.join("main.rs"), b"fn staged() {}\n").unwrap();
        command(root, &["add", "."]);
        fs::write(root.join("main.rs"), b"fn unstaged() {}\n").unwrap();
        let program = executable(root, "cat before/main.rs after/main.rs");
        let diff = ExactDiff::load(root, Path::new("main.rs"), DiffSide::Index).unwrap();
        assert_eq!(
            diff.structural_using(80, None, program.as_os_str())
                .unwrap()
                .unwrap(),
            "fn original() {}\nfn staged() {}\n"
        );
        let diff = ExactDiff::load(root, Path::new("main.rs"), DiffSide::Worktree).unwrap();
        assert_eq!(
            diff.structural_using(80, None, program.as_os_str())
                .unwrap()
                .unwrap(),
            "fn staged() {}\nfn unstaged() {}\n"
        );
    }

    #[test]
    fn missing_difftastic_is_an_ordinary_fallback() {
        let directory = fixture();
        let root = directory.path();
        fs::write(root.join("main.rs"), b"fn main() {}\n").unwrap();
        let diff = ExactDiff::load(root, Path::new("main.rs"), DiffSide::Worktree).unwrap();
        assert!(
            diff.structural_using(80, None, root.join("no-such-program").as_os_str())
                .unwrap()
                .is_none()
        );
    }
}
