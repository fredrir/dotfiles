//! Finding the files a run is about.
//!
//! Every file the walk reaches is a candidate. Which of them any tool wants is
//! settled afterwards — by the provider table for the nine rows that go by
//! extension, and by asking dotfmt for the tenth, whose answer depends on
//! `include`/`exclude` patterns this crate deliberately does not model.
//!
//! The walk is hand-rolled rather than delegated to the `ignore` crate, and
//! the reason is in this repository's own `.gitignore`: line 68 is `**/lua`.
//! Git does not apply ignore rules to paths it already tracks, so the forty
//! `.lua` files under `shared/nvim/lua/` are not ignored — but a crate that
//! reads the ignore files without reading the index has no way to know that,
//! and would silently hand stylua an empty list. So the tree is walked here
//! and the candidates are put through one `git check-ignore` subprocess,
//! which is git's real answer, index rule and all.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::lang::Lang;

/// Directories that never hold anything worth formatting, and mostly hold so
/// much that walking them is the whole cost of a run.
pub const SKIP: [&str; 22] = [
    ".git",
    ".jj",
    ".hg",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".next",
    ".turbo",
    "vendor",
    ".terraform",
    ".gradle",
    ".idea",
    ".direnv",
    ".cache",
];

/// How deep the walk goes before it stops descending.
pub const MAX_DEPTH: usize = 64;

/// How many candidates a run will carry. A tree larger than this is a
/// mistaken target far more often than it is a real one.
pub const MAX_FILES: usize = 200_000;

/// Lockfiles, matched by name wherever they are found.
///
/// These are written by a resolver rather than by a person, and reformatting
/// one only picks a fight with whatever wrote it: the next `npm install` or
/// `cargo build` puts it straight back, and every run in between reports drift
/// that nobody introduced. It is the same problem the `.dotfile` generators
/// have, and the same answer — leave what a machine writes to the machine that
/// writes it.
pub const LOCKFILES: [&str; 13] = [
    "lazy-lock.json",
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    "deno.lock",
    "Cargo.lock",
    "uv.lock",
    "poetry.lock",
    "Gemfile.lock",
    "composer.lock",
    "flake.lock",
];

/// What the walk found, and what it could not reach.
#[derive(Default)]
pub struct Found {
    /// Every candidate, relative to the root, sorted — whatever its name.
    ///
    /// Sorting by extension is left to the provider table and to dotfmt,
    /// because dotfmt owns files by path as well as by extension: `_empty_`
    /// under `**ssh/` is a filename with no extension at all, and a walk that
    /// kept only the extensions this table knows could never offer it.
    pub files: Vec<PathBuf>,
    /// Directories that could not be read, so what is in them is not here.
    pub unreadable: usize,
    /// Directories left undescended by the depth cap.
    pub deep: usize,
    /// Whether the file cap was reached, so the list is short of the truth.
    pub capped: bool,
    /// Lockfiles a provider owns the extension of, left where they are.
    pub lockfiles: Vec<PathBuf>,
}

impl Found {
    fn absorb(&mut self, other: Found) {
        self.files.extend(other.files);
        self.unreadable += other.unreadable;
        self.deep += other.deep;
        self.capped |= other.capped;
        self.lockfiles.extend(other.lockfiles);
    }
}

/// Every file under `root`, less the skip list, the lockfiles, and anything
/// on the other side of a symlink.
pub fn walk(root: &Path) -> Found {
    let seen = AtomicUsize::new(0);
    let mut found = read(root, PathBuf::new(), 0, &seen);
    found.files.sort();
    found.lockfiles.sort();
    found
}

/// Each directory's subdirectories are read in parallel: the walk spends its
/// time waiting on directory reads, and a thread per core hides most of that
/// on a large tree.
fn read(path: &Path, prefix: PathBuf, depth: usize, seen: &AtomicUsize) -> Found {
    let Ok(listing) = fs::read_dir(path) else {
        return Found {
            unreadable: 1,
            ..Found::default()
        };
    };
    let mut found = Found::default();
    let mut below = Vec::new();
    for entry in listing {
        // An entry that fails mid-listing is one this walk cannot see, which
        // is the same thing an unreadable directory is: say so, rather than
        // report a tree smaller than it is.
        let Ok(entry) = entry else {
            found.unreadable += 1;
            continue;
        };
        // `file_type` reads the directory entry rather than what it points
        // at, so a symlink is never mistaken for what is on the other end.
        // That keeps a link loop from being a hang, and it keeps a link to a
        // file somewhere else from being rewritten by a run that was aimed at
        // this tree — a target named outright still reaches such a file.
        let Ok(kind) = entry.file_type() else {
            found.unreadable += 1;
            continue;
        };
        let name = entry.file_name();
        if kind.is_dir() {
            if skipped(&name) {
                continue;
            }
            if depth + 1 > MAX_DEPTH {
                found.deep += 1;
                continue;
            }
            below.push((entry.path(), prefix.join(&name)));
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let relative = prefix.join(&name);
        // Set aside rather than dropped, so a run can say which generated
        // files it left where they were.
        if locked(&name) {
            found.lockfiles.push(relative);
            continue;
        }
        if seen.fetch_add(1, Ordering::Relaxed) >= MAX_FILES {
            found.capped = true;
            continue;
        }
        found.files.push(relative);
    }
    let deeper: Vec<Found> = below
        .into_par_iter()
        .map(|(path, prefix)| read(&path, prefix, depth + 1, seen))
        .collect();
    for other in deeper {
        found.absorb(other);
    }
    found
}

fn skipped(name: &OsStr) -> bool {
    SKIP.iter().any(|skip| OsStr::new(skip) == name)
}

fn locked(name: &OsStr) -> bool {
    LOCKFILES.iter().any(|lock| OsStr::new(lock) == name)
}

// --------------------------------------------------- files encrypted at rest

/// How much of a file is read looking for the markers, at each end.
pub const SNIFF: usize = 8192;

/// What SOPS wraps every encrypted value in, whatever format it wrote.
const CIPHERTEXT: &str = "ENC[AES256_GCM";

/// The fields a real one carries, all of them required.
///
/// The marker on its own is not evidence: this repository's own secret
/// scanner keeps it in a string literal (`patterns.py`), one of its tests
/// builds half of one, and the file you are reading names it twice. Skipping
/// those would quietly stop formatting four source files, which is the same
/// silent gap an empty dotfmt row would have been.
const FIELDS: [&str; 4] = [",data:", ",iv:", ",tag:", ",type:"];

/// Take the encrypted candidates out of the rows they landed in, and say
/// which they were.
///
/// This runs over the rows rather than over the walk, so the only files opened
/// are the ones some provider was about to rewrite — and so a file named
/// outright on the command line is covered too. There is no safe way to
/// reformat one of these: at best it is a diff the size of the file on every
/// run, and at worst it breaks the MAC that guards it.
pub fn drop_encrypted(root: &Path, work: &mut Vec<(Lang, Vec<PathBuf>)>) -> Vec<PathBuf> {
    let candidates: Vec<PathBuf> = work
        .iter()
        .flat_map(|(_, files)| files.iter().cloned())
        .collect();
    let sealed = encrypted(root, &candidates);
    if sealed.is_empty() {
        return sealed;
    }
    let named: HashSet<&OsStr> = sealed.iter().map(|path| path.as_os_str()).collect();
    for (_, files) in work.iter_mut() {
        files.retain(|file| !named.contains(file.as_os_str()));
    }
    work.retain(|(_, files)| !files.is_empty());
    sealed
}

/// Which of these are encrypted, read rather than matched by name.
///
/// Nothing forces a project to call an encrypted file `*.enc.yaml`, and the
/// name that does follow the convention here is the wrong one: `.sops.yaml`
/// is the configuration saying which keys to encrypt *with*, is not itself
/// encrypted, and must still be formatted. Only the contents can tell the two
/// apart.
pub fn encrypted(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut sealed: Vec<PathBuf> = files
        .par_iter()
        .filter(|file| is_encrypted(&root.join(file)))
        .cloned()
        .collect();
    sealed.sort();
    sealed
}

/// Both ends of the file are read, and for anything up to two windows long
/// they overlap and the whole of it is read.
///
/// Both ends matter. In YAML the metadata block is appended, so it is at the
/// end; the ciphertext is usually at the start, but SOPS can be told to
/// encrypt only some values, and then a long plaintext head pushes the first
/// `ENC[` out of reach.
fn is_encrypted(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let Ok(size) = file.metadata().map(|meta| meta.len()) else {
        return false;
    };
    if marked(&window(&mut file, 0)) {
        return true;
    }
    size > SNIFF as u64 && marked(&window(&mut file, size - SNIFF as u64))
}

fn window(file: &mut fs::File, from: u64) -> String {
    if file.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::with_capacity(SNIFF);
    if file.take(SNIFF as u64).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn marked(window: &str) -> bool {
    window
        .lines()
        .any(|line| metadata(line) || ciphertext(line))
}

/// The block SOPS adds to every file it encrypts, in each of the formats it
/// writes.
///
/// This is the defining mark rather than a hint: a document without it holds
/// no key material and no MAC, so it cannot be decrypted and is not one of
/// these files whatever else it contains. It is also the mark that source
/// code about SOPS does not accidentally carry.
///
/// A document that really does keep unrelated settings under a top-level
/// `sops` key would go unformatted and be named under `--verbose`. That is a
/// far cheaper mistake than rewriting a secret.
fn metadata(line: &str) -> bool {
    // YAML and INI put it at the top level, so at column zero. An indented
    // `sops:` belongs to something else.
    if line.trim_end() == "sops:" || line.trim_end() == "[sops]" {
        return true;
    }
    // dotenv cannot nest, so the block is flattened into prefixed keys.
    if line.starts_with("sops_version=") || line.starts_with("sops_mac=") {
        return true;
    }
    // JSON, at whatever depth the writer indented it to.
    line.trim_start().starts_with("\"sops\":")
}

/// One complete encrypted value, every field present.
///
/// The second mark, for a fragment of a document separated from its metadata.
/// Requiring the whole shape is what keeps the marker written out in source
/// from reading as a secret.
fn ciphertext(line: &str) -> bool {
    line.contains(CIPHERTEXT) && FIELDS.iter().all(|field| line.contains(field))
}

/// Drop the candidates git would ignore.
///
/// One subprocess for the whole list: `check-ignore` reads paths on stdin and
/// writes back only the ones it ignores, so the answer is a set difference.
/// Outside a work tree it exits 128 and nothing is dropped, which is the right
/// answer for a directory git knows nothing about.
pub fn drop_ignored(root: &Path, files: Vec<PathBuf>) -> Vec<PathBuf> {
    if files.is_empty() {
        return files;
    }
    let Some(ignored) = ask_git(root, &files) else {
        return files;
    };
    files
        .into_iter()
        .filter(|path| !ignored.contains(path.as_os_str().as_encoded_bytes()))
        .collect()
}

/// `None` when git could not answer at all, which is not the same as an
/// answer of "nothing is ignored".
fn ask_git(root: &Path, files: &[PathBuf]) -> Option<HashSet<Vec<u8>>> {
    let mut child = Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // The paths go out on a thread of their own. git writes its answer while
    // it is still reading the question, so a run that wrote the whole list
    // first would deadlock against a full pipe on any tree worth walking.
    let mut sink = child.stdin.take()?;
    let payload: Vec<u8> = files
        .iter()
        .flat_map(|path| {
            let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
            bytes.push(0);
            bytes
        })
        .collect();
    let feeding = std::thread::spawn(move || {
        sink.write_all(&payload).ok();
    });

    let mut answer = Vec::new();
    child.stdout.take()?.read_to_end(&mut answer).ok()?;
    feeding.join().ok()?;
    let status = child.wait().ok()?;
    // 0 is "some are ignored", 1 is "none are"; anything else — no repository,
    // a broken index — means the answer cannot be trusted as a whole.
    if !matches!(status.code(), Some(0 | 1)) {
        return None;
    }
    Some(
        answer
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(<[u8]>::to_vec)
            .collect(),
    )
}
