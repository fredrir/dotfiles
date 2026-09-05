use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use rayon::prelude::*;
use workstation::walk::{Policy, Symlinks};

use crate::lang::Lang;

pub const MAX_DEPTH: usize = 64;

pub const MAX_FILES: usize = 200_000;

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

#[derive(Default)]
pub struct Found {
    pub files: Vec<PathBuf>,
    pub unreadable: usize,
    pub deep: usize,
    pub capped: bool,
    pub lockfiles: Vec<PathBuf>,
}

pub fn walk(root: &Path) -> Found {
    let policy = Policy::new()
        .max_depth(MAX_DEPTH)
        .max_files(MAX_FILES)
        .symlinks(Symlinks::Drop);
    let set_aside: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let walked = workstation::walk::walk(root, &policy, |_, entries| {
        let mut files = Vec::new();
        for entry in entries.iter().filter(|entry| entry.is_file()) {
            // Set aside rather than dropped, so a run can say which generated
            // files it left where they were.
            if locked(&entry.name) {
                let mut kept = set_aside.lock().unwrap_or_else(|held| held.into_inner());
                kept.push(entry.relative.clone());
                continue;
            }
            files.push(entry.relative.clone());
        }
        files
    });
    let mut found = Found {
        files: walked.items,
        unreadable: walked.unreadable,
        deep: walked.deep,
        capped: walked.capped,
        lockfiles: set_aside
            .into_inner()
            .unwrap_or_else(|held| held.into_inner()),
    };
    found.files.sort();
    found.lockfiles.sort();
    found
}

fn locked(name: &OsStr) -> bool {
    LOCKFILES.iter().any(|lock| OsStr::new(lock) == name)
}

// --------------------------------------------------- files encrypted at rest

pub const SNIFF: usize = 8192;

const CIPHERTEXT: &str = "ENC[AES256_GCM";

const FIELDS: [&str; 4] = [",data:", ",iv:", ",tag:", ",type:"];

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

pub fn encrypted(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut sealed: Vec<PathBuf> = files
        .par_iter()
        .filter(|file| is_encrypted(&root.join(file)))
        .cloned()
        .collect();
    sealed.sort();
    sealed
}

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

fn ciphertext(line: &str) -> bool {
    line.contains(CIPHERTEXT) && FIELDS.iter().all(|field| line.contains(field))
}

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
