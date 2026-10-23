use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError {
    pub message: String,
    pub status: Option<i32>,
}

impl GitError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

impl From<std::io::Error> for GitError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub label: String,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub mutates: bool,
    literal_paths: bool,
}

impl CommandSpec {
    pub fn new(
        label: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            label: label.into(),
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            mutates: true,
            literal_paths: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    cancellation: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Debug)]
pub struct DiscardGuard {
    pub paths: Vec<PathBuf>,
    root: PathBuf,
    index_path: PathBuf,
    index: IndexIdentity,
    files: Vec<Option<FileIdentity>>,
    commands: Vec<CommandSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    digest: [u8; 32],
    symlink: bool,
    permissions: std::fs::Permissions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexIdentity {
    entry: Option<FileIdentity>,
    target: Option<(PathBuf, Option<FileIdentity>)>,
}

impl DiscardGuard {
    pub fn capture(repo: &Repo, paths: &[PathBuf]) -> Result<Self, GitError> {
        let commands = repo.discard(paths)?;
        let index_path = path_from_bytes(trim_newline(&repo.read([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
        ])?))?;
        let index = index_identity(&index_path, repo.cancellation.as_deref())?;
        let files = paths
            .iter()
            .map(|path| file_identity(&repo.root.join(path), repo.cancellation.as_deref()))
            .collect::<Result<_, _>>()?;
        if index_identity(&index_path, repo.cancellation.as_deref())? != index {
            return Err(GitError::new(
                "The index changed while preparing the discard; try again",
            ));
        }
        Ok(Self {
            paths: paths.to_vec(),
            root: repo.root.clone(),
            index_path,
            index,
            files,
            commands,
        })
    }

    pub fn commands(&self, repo: &Repo) -> Result<Vec<CommandSpec>, GitError> {
        if repo.root != self.root
            || index_identity(&self.index_path, repo.cancellation.as_deref())? != self.index
        {
            return Err(GitError::new(
                "The repository changed after confirmation; review the changes again",
            ));
        }
        for (path, expected) in self.paths.iter().zip(&self.files) {
            if file_identity(&repo.root.join(path), repo.cancellation.as_deref())? != *expected {
                return Err(GitError::new(format!(
                    "{} changed after confirmation; review it again",
                    path.display()
                )));
            }
        }
        Ok(self.commands.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationGuard {
    pub operation: Operation,
    root: PathBuf,
    head: Option<String>,
    metadata: Vec<(PathBuf, OperationStamp)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OperationStamp {
    identity: Option<FileIdentity>,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl OperationGuard {
    pub fn capture(repo: &Repo) -> Result<Self, GitError> {
        let operation = repo
            .operation()?
            .ok_or_else(|| GitError::new("No operation in progress"))?;
        let mut metadata = Vec::new();
        for name in [
            "rebase-merge",
            "rebase-apply",
            "sequencer",
            "MERGE_HEAD",
            "MERGE_MSG",
            "MERGE_MODE",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "ORIG_HEAD",
            "HEAD",
        ] {
            operation_metadata(
                &repo.git_dir.join(name),
                repo.cancellation.as_deref(),
                &mut metadata,
            )?;
        }
        Ok(Self {
            operation,
            root: repo.root.clone(),
            head: repo.status()?.oid,
            metadata,
        })
    }

    pub fn abort(&self, repo: &Repo) -> Result<CommandSpec, GitError> {
        if Self::capture(repo)? != *self {
            return Err(GitError::new(
                "The Git operation changed; review it before aborting",
            ));
        }
        repo.abort_operation()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub branches: Vec<Branch>,
    pub commits: Vec<Commit>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub operation: Option<Operation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: Option<String>,
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub entries: Vec<StatusEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index: char,
    pub worktree: char,
    pub unmerged: bool,
    pub untracked: bool,
    pub submodule: Option<String>,
}

impl StatusEntry {
    pub fn staged(&self) -> bool {
        !self.untracked && self.index != '.' && self.index != ' '
    }

    pub fn unstaged(&self) -> bool {
        self.untracked || (self.worktree != '.' && self.worktree != ' ')
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub full_name: String,
    pub oid: String,
    pub current: bool,
    pub remote: bool,
    pub upstream: Option<String>,
    pub upstream_remote: Option<String>,
    pub upstream_ref: Option<String>,
    pub upstream_gone: bool,
    pub tracking: (usize, usize),
    pub divergence: Option<(usize, usize)>,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
}

impl Branch {
    pub fn remote_parts(&self) -> Option<(&str, &str)> {
        self.name.split_once('/').filter(|_| self.remote)
    }
}

pub type LineStat = (PathBuf, Option<(usize, usize)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub kind: String,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub timestamp: i64,
    pub subject: String,
    pub decorations: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    pub reference: String,
    pub oid: String,
    pub subject: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub oid: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Bisect => "bisect",
        })
    }
}

impl Repo {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let path = path.as_ref();
        let mut command = Command::new("git");
        command.arg("-C").arg(path).args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
        ]);
        let root = path_from_bytes(trim_newline(&capture(&mut command, None)?))?;
        let mut repo = Self {
            root,
            git_dir: PathBuf::new(),
            cancellation: None,
        };
        repo.git_dir = path_from_bytes(trim_newline(&repo.read([
            "rev-parse",
            "--path-format=absolute",
            "--absolute-git-dir",
        ])?))?;
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_cancel(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn command(&self, spec: &CommandSpec) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(&self.root)
            .args(["--no-pager", "-c", "color.ui=false"]);
        if spec.literal_paths {
            command.arg("--literal-pathspecs");
        }
        if !spec.mutates {
            command.arg("--no-optional-locks");
        }
        command.args(&spec.args);
        command
    }

    fn read(
        &self,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Result<Vec<u8>, GitError> {
        let mut spec = CommandSpec::new("Read repository", args);
        spec.mutates = false;
        spec.literal_paths = true;
        capture(&mut self.command(&spec), self.cancellation.as_deref())
    }

    pub fn snapshot(&self, limit: usize) -> Result<Snapshot, GitError> {
        let status = self.status()?;
        let commits = self.commits(limit)?;
        Ok(Snapshot {
            branches: self.branches(status.oid.is_some())?,
            status,
            commits,
            stashes: self.stashes()?,
            worktrees: self.worktrees()?,
            operation: self.operation()?,
        })
    }

    pub fn status(&self) -> Result<Status, GitError> {
        parse_status(&self.read([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])?)
    }

    pub fn files(&self) -> Result<Vec<PathBuf>, GitError> {
        let bytes = self.read([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])?;
        bytes
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(path_from_bytes)
            .collect::<Result<BTreeSet<_>, _>>()
            .map(|paths| paths.into_iter().collect())
    }

    pub fn branches(&self, head: bool) -> Result<Vec<Branch>, GitError> {
        const FIELDS: usize = 11;
        let output = self.read([
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%00%(objectname)%00%(HEAD)%00%(upstream:short)%00%(symref)%00%(upstream:track,nobracket)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(committerdate:unix)%00%(authorname)%00%(contents:subject)%00",
            "refs/heads/",
            "refs/remotes/",
        ])?;
        let divergence = if head {
            self.divergence()
        } else {
            BTreeMap::new()
        };
        let fields: Vec<_> = output.split(|byte| *byte == 0).collect();
        let mut branches = Vec::new();
        for fields in fields.chunks(FIELDS) {
            if fields.len() != FIELDS {
                if fields.iter().all(|field| field.trim_ascii().is_empty()) {
                    continue;
                }
                return Err(GitError::new("Malformed branch data"));
            }
            let full_name = strict_text(fields[0].trim_ascii_start(), "branch name")?;
            let remote = full_name.starts_with("refs/remotes/");
            if remote && !fields[4].is_empty() {
                continue;
            }
            let name = full_name
                .strip_prefix("refs/heads/")
                .or_else(|| full_name.strip_prefix("refs/remotes/"))
                .unwrap_or(&full_name)
                .to_owned();
            let track = text(fields[5]);
            let mut tracking = (0, 0);
            for part in track.split(", ") {
                if let Some(count) = part.strip_prefix("ahead ") {
                    tracking.0 = count.parse().unwrap_or(0);
                } else if let Some(count) = part.strip_prefix("behind ") {
                    tracking.1 = count.parse().unwrap_or(0);
                }
            }
            branches.push(Branch {
                name,
                oid: strict_text(fields[1], "object ID")?,
                current: fields[2] == b"*",
                remote,
                upstream: nonempty_text(fields[3]),
                upstream_remote: nonempty_text(fields[6]),
                upstream_ref: nonempty_text(fields[7]),
                upstream_gone: track == "gone",
                tracking,
                divergence: divergence.get(&full_name).copied(),
                timestamp: text(fields[8]).parse().unwrap_or(0),
                author: text(fields[9]),
                subject: text(fields[10]),
                full_name,
            });
        }
        Ok(branches)
    }

    fn divergence(&self) -> BTreeMap<String, (usize, usize)> {
        let Ok(output) = self.read([
            "for-each-ref",
            "--format=%(refname)%00%(ahead-behind:HEAD)",
            "refs/heads/",
            "refs/remotes/",
        ]) else {
            return BTreeMap::new();
        };
        output
            .split(|byte| *byte == b'\n')
            .filter_map(|line| {
                let (reference, counts) = text(line)
                    .split_once('\0')
                    .map(|(reference, counts)| (reference.to_owned(), counts.to_owned()))?;
                let (ahead, behind) = counts.split_once(' ')?;
                Some((reference, (ahead.parse().ok()?, behind.parse().ok()?)))
            })
            .collect()
    }

    pub fn commits(&self, limit: usize) -> Result<Vec<Commit>, GitError> {
        parse_commits(&self.read([
            OsString::from("log"),
            OsString::from("--exclude=refs/stash"),
            OsString::from("--all"),
            OsString::from("--topo-order"),
            OsString::from("--date-order"),
            OsString::from("--no-show-signature"),
            OsString::from("-z"),
            OsString::from(format!("--max-count={limit}")),
            OsString::from(COMMIT_FORMAT),
            OsString::from("--"),
        ])?)
    }

    pub fn range_commits(
        &self,
        base: &str,
        tip: &str,
        limit: usize,
    ) -> Result<Vec<Commit>, GitError> {
        self.log(format!("{base}..{tip}"), limit)
    }

    pub fn recent_commits(&self, tip: &str, limit: usize) -> Result<Vec<Commit>, GitError> {
        self.log(tip.into(), limit)
    }

    fn log(&self, revision: String, limit: usize) -> Result<Vec<Commit>, GitError> {
        parse_commits(&self.read([
            "log".into(),
            "--no-show-signature".into(),
            "-z".into(),
            format!("--max-count={limit}"),
            COMMIT_FORMAT.into(),
            "--end-of-options".into(),
            revision,
            "--".into(),
        ])?)
    }

    pub fn find_commit(&self, oid: &str) -> Result<Option<Commit>, GitError> {
        Ok(parse_commits(&self.read([
            "log",
            "--no-show-signature",
            "-z",
            "--max-count=1",
            COMMIT_FORMAT,
            "--end-of-options",
            oid,
            "--",
        ])?)?
        .pop())
    }

    pub fn count_range(&self, base: &str, tip: &str) -> Result<usize, GitError> {
        text(&self.read([
            "rev-list".into(),
            "--count".into(),
            "--end-of-options".into(),
            format!("{base}..{tip}"),
            "--".into(),
        ])?)
        .trim()
        .parse()
        .map_err(|_| GitError::new("Malformed commit count"))
    }

    pub fn merge_base(&self, left: &str, right: &str) -> Result<Option<String>, GitError> {
        match self.read(["merge-base", "--end-of-options", left, right]) {
            Ok(output) => Ok(nonempty_text(trim_newline(&output))),
            Err(error) if error.status == Some(1) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn tree_paths(&self, revision: &str) -> Result<Vec<PathBuf>, GitError> {
        self.read([
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            "--full-tree",
            "--end-of-options",
            revision,
        ])?
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(path_from_bytes)
        .collect()
    }

    pub fn line_stats(&self, from: &str, to: &str) -> Result<Vec<LineStat>, GitError> {
        let output = self.read([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--numstat",
            "-z",
            "--end-of-options",
            from,
            to,
            "--",
        ])?;
        output
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| {
                let mut fields = record.splitn(3, |byte| *byte == b'\t');
                let added = text(fields.next().unwrap_or_default()).parse().ok();
                let removed = text(fields.next().unwrap_or_default()).parse().ok();
                let path = fields
                    .next()
                    .ok_or_else(|| GitError::new("Malformed diff statistics"))?;
                Ok((path_from_bytes(path)?, added.zip(removed)))
            })
            .collect()
    }

    pub fn tree_entry(&self, revision: &str, path: &Path) -> Result<Option<TreeEntry>, GitError> {
        validate_path(path)?;
        let output = self.read([
            OsStr::new("ls-tree"),
            OsStr::new("-l"),
            OsStr::new("-z"),
            OsStr::new("--full-tree"),
            OsStr::new("--end-of-options"),
            OsStr::new(revision),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        let Some(header) = output
            .split(|byte| *byte == b'\t')
            .next()
            .filter(|_| !output.is_empty())
        else {
            return Ok(None);
        };
        let header = text(header);
        let mut fields = header.split_whitespace();
        Ok(Some(TreeEntry {
            mode: fields.next().unwrap_or_default().to_owned(),
            kind: fields.next().unwrap_or_default().to_owned(),
            size: fields.nth(1).and_then(|size| size.parse().ok()),
        }))
    }

    pub fn blob(&self, revision: &str, path: &Path) -> Result<Vec<u8>, GitError> {
        validate_path(path)?;
        let mut object = OsString::from(revision);
        object.push(":");
        object.push(path);
        self.read([OsString::from("cat-file"), OsString::from("blob"), object])
    }

    pub fn commit_details(&self, oid: &str) -> Result<crate::commit::CommitDetails, GitError> {
        let output = self.read([
            "show",
            "--no-patch",
            "--no-show-signature",
            "--format=%H%x00%an%x00%at%x00%aI%x00%G?%x00%s%x00%ae%x00%B",
            oid,
            "--",
        ])?;
        let fields: Vec<_> = output.splitn(8, |byte| *byte == 0).collect();
        if fields.len() != 8 {
            return Err(GitError::new("Malformed commit details"));
        }
        let mut signature = text(fields[4]);
        if matches!(signature.as_str(), "N" | "E") {
            let object = self.read(["cat-file", "commit", oid])?;
            if object
                .split(|byte| *byte == b'\n')
                .take_while(|line| !line.is_empty())
                .any(|line| line.starts_with(b"gpgsig ") || line.starts_with(b"gpgsig-sha256 "))
            {
                signature = "S".into();
            }
        }
        if matches!(signature.as_str(), "S" | "U") && self.verify_configured_ssh_key(oid) {
            signature = "G".into();
        }
        Ok(crate::commit::CommitDetails {
            oid: strict_text(fields[0], "object ID")?,
            author: text(fields[1]),
            timestamp: text(fields[2])
                .parse()
                .map_err(|_| GitError::new("Malformed commit time"))?,
            authored_iso: text(fields[3]),
            signature: Some(signature),
            subject: text(fields[5]),
            author_email: text(fields[6]),
            message: Some(text(fields[7]).trim_end_matches('\n').to_owned()),
            github: None,
            github_signature_verified: None,
        })
    }

    fn verify_configured_ssh_key(&self, oid: &str) -> bool {
        if self
            .read(["config", "--get", "gpg.ssh.allowedSignersFile"])
            .is_ok_and(|value| !text(&value).trim().is_empty())
        {
            return false;
        }
        let verify = || -> Option<bool> {
            let configured = text(
                &self
                    .read(["config", "--path", "--get", "user.signingkey"])
                    .ok()?,
            );
            let configured = configured.trim();
            let public = if let Some(key) = configured.strip_prefix("key::") {
                key.to_owned()
            } else if configured.starts_with("ssh-")
                || configured.starts_with("ecdsa-")
                || configured.starts_with("sk-")
            {
                configured.to_owned()
            } else {
                let mut path = PathBuf::from(configured);
                if path.extension() != Some(OsStr::new("pub")) {
                    let mut public_path = path.into_os_string();
                    public_path.push(".pub");
                    path = public_path.into();
                }
                let path = if path.is_absolute() {
                    path
                } else {
                    self.root.join(path)
                };
                let mut public = String::new();
                std::fs::File::open(path)
                    .ok()?
                    .take(16 * 1024 + 1)
                    .read_to_string(&mut public)
                    .ok()?;
                if public.len() > 16 * 1024 {
                    return None;
                }
                public
            };
            let mut fields = public.split_whitespace();
            let algorithm = fields.next()?;
            let key = fields.next()?;
            if !(algorithm.starts_with("ssh-")
                || algorithm.starts_with("ecdsa-")
                || algorithm.starts_with("sk-"))
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
            {
                return None;
            }
            let mut allowed = tempfile::NamedTempFile::new().ok()?;
            writeln!(
                allowed,
                "dogit-configured-key namespaces=\"git\" {algorithm} {key}"
            )
            .ok()?;
            let mut setting = OsString::from("gpg.ssh.allowedSignersFile=");
            setting.push(allowed.path());
            let result = self
                .read([
                    OsString::from("-c"),
                    setting,
                    "show".into(),
                    "--no-patch".into(),
                    "--no-show-signature".into(),
                    "--format=%G?".into(),
                    oid.into(),
                    "--".into(),
                ])
                .ok()?;
            Some(text(&result).trim() == "G")
        };
        verify().unwrap_or(false)
    }

    pub(crate) fn github_repository(&self) -> Option<String> {
        let output = self
            .read(["config", "--get-regexp", "^remote\\..*\\.url$"])
            .ok()?;
        let remotes = text(&output);
        let mut lines: Vec<_> = remotes.lines().collect();
        lines.sort_by_key(|line| !line.starts_with("remote.origin.url "));
        lines.iter().find_map(|line| {
            let (_, url) = line.split_once(' ')?;
            crate::commit::github_repository(url)
        })
    }

    pub fn stashes(&self) -> Result<Vec<Stash>, GitError> {
        let output = self.read(["stash", "list", "-z", "--format=%gd%x00%H%x00%s"])?;
        let fields: Vec<_> = output.split(|byte| *byte == 0).collect();
        let mut stashes = Vec::new();
        for fields in fields[..fields.len().saturating_sub(1)].chunks(3) {
            if fields.len() != 3 {
                return Err(GitError::new("Malformed stash data"));
            }
            stashes.push(Stash {
                reference: strict_text(fields[0], "stash reference")?,
                oid: strict_text(fields[1], "object ID")?,
                subject: text(fields[2]),
            });
        }
        Ok(stashes)
    }

    pub fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        let output = self.read(["worktree", "list", "--porcelain", "-z"])?;
        let mut worktrees = Vec::new();
        let mut current = Worktree::default();
        for field in output.split(|byte| *byte == 0) {
            if field.is_empty() {
                if !current.path.as_os_str().is_empty() {
                    worktrees.push(std::mem::take(&mut current));
                }
            } else if let Some(path) = field.strip_prefix(b"worktree ") {
                current.path = path_from_bytes(path)?;
            } else if let Some(oid) = field.strip_prefix(b"HEAD ") {
                current.oid = Some(text(oid));
            } else if let Some(branch) = field.strip_prefix(b"branch ") {
                current.branch = Some(text(branch).trim_start_matches("refs/heads/").to_owned());
            } else if field == b"detached" {
                current.detached = true;
            } else if field == b"locked" {
                current.locked = Some(String::new());
            } else if let Some(reason) = field.strip_prefix(b"locked ") {
                current.locked = Some(text(reason));
            } else if field == b"prunable" {
                current.prunable = Some(String::new());
            } else if let Some(reason) = field.strip_prefix(b"prunable ") {
                current.prunable = Some(text(reason));
            }
        }
        Ok(worktrees)
    }

    pub fn operation(&self) -> Result<Option<Operation>, GitError> {
        for (name, operation) in [
            ("rebase-merge", Operation::Rebase),
            ("rebase-apply", Operation::Rebase),
            ("MERGE_HEAD", Operation::Merge),
            ("CHERRY_PICK_HEAD", Operation::CherryPick),
            ("REVERT_HEAD", Operation::Revert),
            ("BISECT_LOG", Operation::Bisect),
        ] {
            if self.git_dir.join(name).try_exists()? {
                return Ok(Some(operation));
            }
        }
        let todo = self.git_dir.join("sequencer/todo");
        if todo.try_exists()? {
            let bytes = std::fs::read(todo)?;
            if bytes.starts_with(b"revert ") {
                return Ok(Some(Operation::Revert));
            }
            if bytes.starts_with(b"pick ") {
                return Ok(Some(Operation::CherryPick));
            }
        }
        Ok(None)
    }

    pub fn verify_revision(&self, revision: &str) -> Result<String, GitError> {
        if revision.is_empty() || revision.contains('\0') {
            return Err(GitError::new("Choose a revision"));
        }
        let output = self.read([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])?;
        let oid = strict_text(trim_newline(&output), "object ID")?;
        if !(oid.len() == 40 || oid.len() == 64)
            || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GitError::new("Git returned an invalid object ID"));
        }
        Ok(oid)
    }

    pub fn compare(&self, from: &str, to: &str, path: Option<&Path>) -> Result<Vec<u8>, GitError> {
        let mut args = vec![
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--binary".into(),
            self.verify_revision(from)?.into(),
            self.verify_revision(to)?.into(),
            "--".into(),
        ];
        if let Some(path) = path {
            validate_path(path)?;
            args.push(path.as_os_str().to_owned());
        }
        self.read(args)
    }

    pub fn changed_paths(&self, from: &str, to: &str) -> Result<Vec<PathBuf>, GitError> {
        let output = self.read([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            &self.verify_revision(from)?,
            &self.verify_revision(to)?,
            "--",
        ])?;
        output
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(path_from_bytes)
            .collect()
    }

    pub fn commit_paths(&self, revision: &str) -> Result<Vec<PathBuf>, GitError> {
        let oid = self.verify_revision(revision)?;
        let output = self.read([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-m",
            "--first-parent",
            "-z",
            &oid,
            "--",
        ])?;
        output
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(path_from_bytes)
            .collect::<Result<BTreeSet<_>, _>>()
            .map(|paths| paths.into_iter().collect())
    }

    pub fn stage(&self, paths: &[PathBuf]) -> Result<CommandSpec, GitError> {
        path_command("Stage", &["add", "--all"], paths)
    }

    pub fn unstage(&self, paths: &[PathBuf]) -> Result<CommandSpec, GitError> {
        let status = self.status()?;
        let paths = expanded_paths(paths, &status)?;
        if status.oid.is_some() {
            path_command("Unstage", &["restore", "--staged"], &paths)
        } else {
            path_command(
                "Unstage",
                &["rm", "--cached", "--force", "--ignore-unmatch"],
                &paths,
            )
        }
    }

    pub fn commit(&self, message: Option<&str>, amend: bool) -> Result<CommandSpec, GitError> {
        let mut spec = CommandSpec::new(if amend { "Amend" } else { "Commit" }, ["commit"]);
        if amend {
            spec.args.push("--amend".into());
        }
        if let Some(message) = message {
            if message.trim().is_empty() {
                return Err(GitError::new("Write a commit message"));
            }
            spec.args.extend(["--message".into(), message.into()]);
        }
        Ok(spec)
    }

    pub fn commit_message(&self, revision: &str) -> Result<String, GitError> {
        let oid = self.verify_revision(revision)?;
        self.read(["show", "--no-patch", "--format=%B", &oid, "--"])
            .map(|bytes| text(&bytes))
    }

    pub fn switch(&self, branch: &str) -> Result<CommandSpec, GitError> {
        self.validate_branch(branch)?;
        Ok(CommandSpec::new("Switch branch", ["switch", "--", branch]))
    }

    pub fn switch_remote(&self, branch: &str) -> Result<CommandSpec, GitError> {
        self.validate_branch(branch)?;
        let reference = format!("refs/remotes/{branch}");
        self.verify_revision(&reference)?;
        Ok(CommandSpec::new(
            "Track remote branch",
            ["switch", "--track", "--", &reference],
        ))
    }

    pub fn create_branch(&self, name: &str, start: Option<&str>) -> Result<CommandSpec, GitError> {
        self.validate_branch(name)?;
        let mut spec = CommandSpec::new("Create branch", ["switch", "--create", name]);
        if let Some(start) = start {
            spec.args.push(self.verify_revision(start)?.into());
        }
        Ok(spec)
    }

    pub fn delete_branch(&self, name: &str, expected: &str) -> Result<CommandSpec, GitError> {
        self.validate_branch(name)?;
        if self.verify_revision(&format!("refs/heads/{name}"))? != expected {
            return Err(GitError::new("Branch moved; review it before deleting"));
        }
        Ok(CommandSpec::new(
            "Delete branch",
            ["branch", "--delete", "--force", "--", name],
        ))
    }

    pub fn delete_remote_branch(
        &self,
        remote: &str,
        branch: &str,
    ) -> Result<CommandSpec, GitError> {
        self.validate_remote(remote)?;
        self.validate_branch(branch)?;
        Ok(CommandSpec::new(
            "Delete remote branch",
            [
                "push",
                "--delete",
                "--",
                remote,
                &format!("refs/heads/{branch}"),
            ],
        ))
    }

    pub fn rename_branch(&self, from: &str, to: &str) -> Result<CommandSpec, GitError> {
        self.validate_branch(from)?;
        self.validate_branch(to)?;
        Ok(CommandSpec::new(
            "Rename branch",
            ["branch", "--move", "--", from, to],
        ))
    }

    pub fn set_upstream(
        &self,
        branch: &str,
        upstream: Option<&str>,
    ) -> Result<CommandSpec, GitError> {
        self.validate_branch(branch)?;
        let Some(upstream) = upstream else {
            return Ok(CommandSpec::new(
                "Unset upstream",
                ["branch", "--unset-upstream", "--", branch],
            ));
        };
        self.verify_revision(upstream)?;
        Ok(CommandSpec::new(
            "Set upstream",
            [
                "branch",
                &format!("--set-upstream-to={upstream}"),
                "--",
                branch,
            ],
        ))
    }

    pub fn push_branch(
        &self,
        branch: &str,
        remote: &str,
        target: &str,
        track: bool,
    ) -> Result<CommandSpec, GitError> {
        self.validate_branch(branch)?;
        self.validate_remote(remote)?;
        let target = target.strip_prefix("refs/heads/").unwrap_or(target);
        self.validate_branch(target)?;
        let mut spec = CommandSpec::new("Push branch", ["push"]);
        if track {
            spec.args.push("--set-upstream".into());
        }
        spec.args.extend([
            "--".into(),
            remote.into(),
            format!("refs/heads/{branch}:refs/heads/{target}").into(),
        ]);
        Ok(spec)
    }

    fn validate_remote(&self, remote: &str) -> Result<(), GitError> {
        if remote.is_empty() || remote.starts_with('-') || remote.contains('\0') {
            return Err(GitError::new("Invalid remote name"));
        }
        self.read(["remote", "get-url", "--", remote])
            .map(|_| ())
            .map_err(|_| GitError::new(format!("Unknown remote {remote}")))
    }

    fn validate_branch(&self, name: &str) -> Result<(), GitError> {
        if name.is_empty() || name.starts_with('-') || name.contains('\0') {
            return Err(GitError::new("Invalid branch name"));
        }
        self.read(["check-ref-format", "--branch", name])?;
        Ok(())
    }

    pub fn fetch(&self) -> CommandSpec {
        CommandSpec::new("Fetch", ["fetch"])
    }
    pub fn pull(&self) -> CommandSpec {
        CommandSpec::new("Pull", ["pull"])
    }
    pub fn push(&self) -> CommandSpec {
        CommandSpec::new("Push", ["push"])
    }

    pub fn stash_save(
        &self,
        message: Option<&str>,
        include_untracked: bool,
    ) -> Result<CommandSpec, GitError> {
        let mut spec = CommandSpec::new("Save stash", ["stash", "push"]);
        if include_untracked {
            spec.args.push("--include-untracked".into());
        }
        if let Some(message) = message {
            if message.contains('\0') {
                return Err(GitError::new("Invalid stash message"));
            }
            spec.args.extend(["--message".into(), message.into()]);
        }
        Ok(spec)
    }

    pub fn stash_apply(&self, reference: &str, pop: bool) -> Result<CommandSpec, GitError> {
        self.validate_stash(reference)?;
        Ok(CommandSpec::new(
            if pop { "Pop stash" } else { "Apply stash" },
            [
                "stash",
                if pop { "pop" } else { "apply" },
                "--index",
                reference,
            ],
        ))
    }

    pub fn stash_drop(&self, reference: &str) -> Result<CommandSpec, GitError> {
        self.validate_stash(reference)?;
        Ok(CommandSpec::new("Drop stash", ["stash", "drop", reference]))
    }

    fn validate_stash(&self, reference: &str) -> Result<(), GitError> {
        let number = reference
            .strip_prefix("stash@{")
            .and_then(|value| value.strip_suffix('}'));
        if number.is_none_or(|value| {
            value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return Err(GitError::new("Choose a stash from the list"));
        }
        self.verify_revision(reference)?;
        Ok(())
    }

    pub fn verify_stash(&self, reference: &str, expected_oid: &str) -> Result<(), GitError> {
        self.validate_stash(reference)?;
        if self.verify_revision(reference)? != expected_oid {
            return Err(GitError::new(
                "The stash list changed; select the stash again",
            ));
        }
        Ok(())
    }

    pub fn cherry_pick(&self, revision: &str) -> Result<CommandSpec, GitError> {
        self.revision_command("Cherry-pick", "cherry-pick", revision)
    }
    pub fn revert(&self, revision: &str) -> Result<CommandSpec, GitError> {
        self.revision_command("Revert", "revert", revision)
    }

    pub fn cherry_pick_mainline(
        &self,
        revision: &str,
        parent: usize,
    ) -> Result<CommandSpec, GitError> {
        self.revision_command_mainline("Cherry-pick merge", "cherry-pick", revision, parent)
    }

    pub fn revert_mainline(&self, revision: &str, parent: usize) -> Result<CommandSpec, GitError> {
        self.revision_command_mainline("Revert merge", "revert", revision, parent)
    }

    fn revision_command_mainline(
        &self,
        label: &str,
        command: &str,
        revision: &str,
        parent: usize,
    ) -> Result<CommandSpec, GitError> {
        let oid = self.verify_revision(revision)?;
        let ancestry = self.read(["rev-list", "--parents", "--max-count=1", &oid])?;
        let parents = ancestry
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .count()
            .saturating_sub(1);
        if parents < 2 {
            return Err(GitError::new("The selected commit is not a merge"));
        }
        if parent == 0 || parent > parents {
            return Err(GitError::new(format!(
                "Choose a parent between 1 and {parents}"
            )));
        }
        Ok(CommandSpec::new(
            label,
            [
                command.to_owned(),
                "--mainline".into(),
                parent.to_string(),
                oid,
            ],
        ))
    }

    pub fn merge(&self, revision: &str) -> Result<CommandSpec, GitError> {
        self.verify_revision(revision)?;
        Ok(CommandSpec::new("Merge", ["merge", "--", revision]))
    }
    pub fn rebase(&self, revision: &str) -> Result<CommandSpec, GitError> {
        self.verify_revision(revision)?;
        Ok(CommandSpec::new("Rebase", ["rebase", "--", revision]))
    }

    fn revision_command(
        &self,
        label: &str,
        command: &str,
        revision: &str,
    ) -> Result<CommandSpec, GitError> {
        Ok(CommandSpec::new(
            label,
            [command.to_owned(), self.verify_revision(revision)?],
        ))
    }

    pub fn continue_operation(&self) -> Result<CommandSpec, GitError> {
        self.operation_command(false)
    }
    pub fn abort_operation(&self) -> Result<CommandSpec, GitError> {
        self.operation_command(true)
    }

    fn operation_command(&self, abort: bool) -> Result<CommandSpec, GitError> {
        let operation = self
            .operation()?
            .ok_or_else(|| GitError::new("No operation in progress"))?;
        if operation == Operation::Bisect {
            return if abort {
                Ok(CommandSpec::new("End bisect", ["bisect", "reset"]))
            } else {
                Err(GitError::new(
                    "Mark the current commit good or bad to continue bisecting",
                ))
            };
        }
        let option = if abort { "--abort" } else { "--continue" };
        Ok(CommandSpec::new(
            format!("{} {operation}", if abort { "Abort" } else { "Continue" }),
            [operation.to_string(), option.to_owned()],
        ))
    }

    pub fn restore(&self, paths: &[PathBuf]) -> Result<CommandSpec, GitError> {
        path_command(
            "Discard unstaged changes",
            &["restore", "--worktree"],
            paths,
        )
    }

    pub fn discard(&self, paths: &[PathBuf]) -> Result<Vec<CommandSpec>, GitError> {
        if paths.is_empty() {
            return Err(GitError::new("Select files to discard"));
        }
        let status = self.status()?;
        let mut tracked = Vec::new();
        let mut untracked = Vec::new();
        for path in paths {
            validate_path(path)?;
            let entry = status
                .entries
                .iter()
                .find(|entry| entry.path == *path)
                .ok_or_else(|| {
                    GitError::new("The selected changes have changed; refresh and try again")
                })?;
            if entry.unmerged || entry.submodule.is_some() {
                return Err(GitError::new(
                    "Resolve conflicts and submodule changes explicitly",
                ));
            }
            if entry.untracked {
                let metadata = std::fs::symlink_metadata(self.root.join(path))?;
                if metadata.is_dir() {
                    return Err(GitError::new(
                        "Select individual untracked files to discard",
                    ));
                }
                untracked.push(path.clone());
            } else if entry.unstaged() {
                tracked.push(path.clone());
            }
        }
        let mut commands = Vec::new();
        if !tracked.is_empty() {
            commands.push(self.restore(&tracked)?);
        }
        if !untracked.is_empty() {
            commands.push(path_command(
                "Discard untracked files",
                &["clean", "--force"],
                &untracked,
            )?);
        }
        Ok(commands)
    }
}

fn expanded_paths(paths: &[PathBuf], status: &Status) -> Result<Vec<PathBuf>, GitError> {
    let mut expanded = BTreeSet::new();
    for path in paths {
        validate_path(path)?;
        expanded.insert(path.clone());
        if let Some(original) = status
            .entries
            .iter()
            .find(|entry| entry.path == *path)
            .and_then(|entry| entry.original_path.as_ref())
        {
            expanded.insert(original.clone());
        }
    }
    Ok(expanded.into_iter().collect())
}

fn operation_metadata(
    path: &Path,
    cancellation: Option<&AtomicBool>,
    output: &mut Vec<(PathBuf, OperationStamp)>,
) -> Result<(), GitError> {
    if cancellation.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Err(GitError::new("Cancelled"));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let directory = metadata.is_dir();
    let stamp = OperationStamp {
        identity: if directory {
            None
        } else {
            file_identity(path, cancellation)?
        },
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    };
    output.push((path.to_owned(), stamp));
    if directory {
        let mut children = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        children.sort();
        for child in children {
            operation_metadata(&child, cancellation, output)?;
        }
    }
    Ok(())
}

fn index_identity(
    path: &Path,
    cancellation: Option<&AtomicBool>,
) -> Result<IndexIdentity, GitError> {
    let entry = file_identity(path, cancellation)?;
    let target = if entry.as_ref().is_some_and(|entry| entry.symlink) {
        let target = std::fs::canonicalize(path)?;
        Some((target.clone(), file_identity(&target, cancellation)?))
    } else {
        None
    };
    Ok(IndexIdentity { entry, target })
}

fn file_identity(
    path: &Path,
    cancellation: Option<&AtomicBool>,
) -> Result<Option<FileIdentity>, GitError> {
    let check_cancelled = || {
        if cancellation.is_some_and(|token| token.load(Ordering::Acquire)) {
            Err(GitError::new("Cancelled"))
        } else {
            Ok(())
        }
    };
    check_cancelled()?;
    let before = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let symlink = before.file_type().is_symlink();
    let mut digest = Sha256::new();
    if symlink {
        digest.update(std::fs::read_link(path)?.as_os_str().as_encoded_bytes());
    } else if before.is_file() {
        let mut file = std::fs::File::open(path)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancelled()?;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    } else {
        return Err(GitError::new(
            "Only individual regular files and symbolic links can be discarded",
        ));
    }
    let after = std::fs::symlink_metadata(path)?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.file_type() != after.file_type()
        || before.permissions() != after.permissions()
    {
        return Err(GitError::new(
            "A selected file changed while it was being read; try again",
        ));
    }
    Ok(Some(FileIdentity {
        digest: digest.finalize().into(),
        symlink,
        permissions: after.permissions(),
    }))
}

fn path_command(label: &str, args: &[&str], paths: &[PathBuf]) -> Result<CommandSpec, GitError> {
    if paths.is_empty() {
        return Err(GitError::new("Select at least one file"));
    }
    let mut spec = CommandSpec::new(label, args.iter().copied());
    spec.literal_paths = true;
    spec.args.push("--".into());
    for path in paths {
        validate_path(path)?;
        spec.args.push(path.as_os_str().to_owned());
    }
    Ok(spec)
}

fn validate_path(path: &Path) -> Result<(), GitError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err(GitError::new("Expected an exact repository-relative path"));
    }
    Ok(())
}

fn capture(command: &mut Command, cancellation: Option<&AtomicBool>) -> Result<Vec<u8>, GitError> {
    let output = hostkit::process::output_cancellable(
        command,
        hostkit::process::CaptureLimits {
            stdout: 64 * 1024 * 1024,
            stderr: 128 * 1024,
        },
        Duration::from_secs(60),
        &|| cancellation.is_some_and(|token| token.load(Ordering::Acquire)),
    )?;
    if !output.status.success() {
        let message = text(&output.stderr).trim().to_owned();
        return Err(GitError {
            message: if message.is_empty() {
                "Git could not complete the operation".to_owned()
            } else {
                message
            },
            status: output.status.code(),
        });
    }
    if output.stdout_truncated {
        return Err(GitError::new(
            "Repository output exceeds the supported read limit",
        ));
    }
    Ok(output.stdout)
}

fn parse_status(bytes: &[u8]) -> Result<Status, GitError> {
    let mut status = Status::default();
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.head ") {
            if value != b"(detached)" {
                status.branch = Some(strict_text(value, "branch name")?);
            }
        } else if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            if value != b"(initial)" {
                status.oid = Some(strict_text(value, "object ID")?);
            }
        } else if let Some(value) = record.strip_prefix(b"# branch.upstream ") {
            status.upstream = Some(strict_text(value, "upstream")?);
        } else if let Some(value) = record.strip_prefix(b"# branch.ab ") {
            let value = text(value);
            let mut counts = value.split_whitespace();
            status.ahead = counts
                .next()
                .and_then(|part| part.strip_prefix('+'))
                .and_then(|part| part.parse().ok())
                .ok_or_else(|| GitError::new("Malformed ahead count"))?;
            status.behind = counts
                .next()
                .and_then(|part| part.strip_prefix('-'))
                .and_then(|part| part.parse().ok())
                .ok_or_else(|| GitError::new("Malformed behind count"))?;
        } else if record.starts_with(b"# ") || record.starts_with(b"! ") {
            continue;
        } else if let Some(path) = record.strip_prefix(b"? ") {
            status.entries.push(StatusEntry {
                path: path_from_bytes(path)?,
                original_path: None,
                index: '?',
                worktree: '?',
                unmerged: false,
                untracked: true,
                submodule: None,
            });
        } else {
            let count = match record.first() {
                Some(b'1') => 9,
                Some(b'2') => 10,
                Some(b'u') => 11,
                _ => return Err(GitError::new("Unknown Git status record")),
            };
            let fields: Vec<_> = record.splitn(count, |byte| *byte == b' ').collect();
            if fields.len() != count || fields[1].len() != 2 || fields[2].len() != 4 {
                return Err(GitError::new("Malformed Git status record"));
            }
            let original_path = if record[0] == b'2' {
                Some(path_from_bytes(
                    records
                        .next()
                        .ok_or_else(|| GitError::new("Missing rename source"))?,
                )?)
            } else {
                None
            };
            status.entries.push(StatusEntry {
                path: path_from_bytes(fields[count - 1])?,
                original_path,
                index: fields[1][0] as char,
                worktree: fields[1][1] as char,
                unmerged: record[0] == b'u',
                untracked: false,
                submodule: if fields[2][0] == b'S' {
                    Some(text(fields[2]))
                } else {
                    None
                },
            });
        }
    }
    status
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(status)
}

const COMMIT_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%s%x00%D%x00%ae";

fn parse_commits(output: &[u8]) -> Result<Vec<Commit>, GitError> {
    let fields: Vec<_> = output.split(|byte| *byte == 0).collect();
    let mut commits = Vec::new();
    for fields in fields[..fields.len().saturating_sub(1)].chunks(7) {
        if fields.len() != 7 {
            return Err(GitError::new("Malformed history data"));
        }
        commits.push(Commit {
            oid: strict_text(fields[0], "object ID")?,
            parents: text(fields[1])
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            author: text(fields[2]),
            timestamp: text(fields[3])
                .parse()
                .map_err(|_| GitError::new("Malformed commit time"))?,
            subject: text(fields[4]),
            decorations: text(fields[5]),
            author_email: text(fields[6]),
        });
    }
    Ok(commits)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn strict_text(bytes: &[u8], kind: &str) -> Result<String, GitError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| GitError::new(format!("Non-UTF-8 {kind} cannot be represented")))
}
fn nonempty_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(text(bytes))
    }
}
fn trim_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, GitError> {
    use std::os::unix::ffi::OsStrExt;
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(GitError::new("Invalid Git path"));
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, GitError> {
    Ok(PathBuf::from(strict_text(bytes, "path")?))
}
