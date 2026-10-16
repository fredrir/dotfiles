use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CACHE_LIMIT: usize = 4 * 1024 * 1024;
const CACHE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CommitMetadata {
    pub author_email: String,
    pub avatar_url: Option<String>,
    pub signature_verified: bool,
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Batch {
    pub repository: String,
    pub commits: BTreeMap<String, CommitMetadata>,
    fetched_at: u64,
}

impl Batch {
    fn read(path: &Path) -> Option<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(CACHE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > CACHE_LIMIT {
            return None;
        }
        let batch: Self = serde_json::from_slice(&bytes).ok()?;
        (now().saturating_sub(batch.fetched_at) < CACHE_AGE.as_secs()).then_some(batch)
    }

    fn merge(&mut self, response: &[u8]) -> Option<()> {
        let values: Vec<serde_json::Value> = serde_json::from_slice(response).ok()?;
        let mut incoming = BTreeMap::new();
        for value in values {
            let Some(oid) = value["sha"].as_str().filter(|oid| valid_oid(oid)) else {
                continue;
            };
            let verification = &value["commit"]["verification"];
            incoming.insert(
                oid.to_owned(),
                CommitMetadata {
                    author_email: value["commit"]["author"]["email"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    avatar_url: value["author"]["avatar_url"]
                        .as_str()
                        .filter(|url| valid_avatar_url(url))
                        .map(str::to_owned),
                    signature_verified: verification["verified"] == true
                        && verification["reason"] == "valid"
                        && verification["signature"]
                            .as_str()
                            .is_some_and(|signature| !signature.is_empty()),
                },
            );
        }
        while !self.commits.is_empty() && self.commits.len() + incoming.len() > 2048 {
            self.commits.pop_first();
        }
        self.commits.extend(incoming);
        self.fetched_at = now();
        Some(())
    }
}

fn cache_path(repository: &str) -> Option<PathBuf> {
    cache_directory()
        .map(|base| base.join(format!("commits-{:x}.json", Sha256::digest(repository))))
}

pub(crate) fn cached(repo: &crate::git::Repo) -> Batch {
    let Some(repository) = repo.github_repository() else {
        return Batch::default();
    };
    cache_path(&repository)
        .as_deref()
        .and_then(Batch::read)
        .filter(|batch| batch.repository == repository)
        .unwrap_or_else(|| Batch {
            repository: repository.clone(),
            ..Default::default()
        })
}

pub(crate) fn load(repo: &crate::git::Repo, oid: &str, cancel: &AtomicBool) -> Batch {
    let mut batch = cached(repo);
    if batch.repository.is_empty() || !valid_oid(oid) {
        return batch;
    }
    if batch.commits.contains_key(oid) {
        return batch;
    }
    let endpoint = format!("repos/{}/commits?sha={oid}&per_page=100", batch.repository);
    let bytes = page(&endpoint, cancel).or_else(|| {
        batch
            .commits
            .is_empty()
            .then(|| {
                page(
                    &format!("repos/{}/commits?per_page=100", batch.repository),
                    cancel,
                )
            })
            .flatten()
    });
    if let Some(bytes) = bytes
        && batch.merge(&bytes).is_some()
        && let Some(path) = cache_path(&batch.repository)
        && let Ok(bytes) = serde_json::to_vec(&batch)
    {
        persist(&path, &bytes);
    }
    batch
}

fn page(endpoint: &str, cancel: &AtomicBool) -> Option<Vec<u8>> {
    capture(
        Command::new("gh")
            .args(["api", "--hostname", "github.com", endpoint])
            .env("GH_PROMPT_DISABLED", "1"),
        CACHE_LIMIT,
        cancel,
    )
    .or_else(|| {
        fetch(
            &format!("https://api.github.com/{endpoint}"),
            CACHE_LIMIT,
            cancel,
        )
    })
}

pub(crate) fn valid_avatar_url(url: &str) -> bool {
    url.starts_with("https://avatars.githubusercontent.com/") && !url.chars().any(char::is_control)
}

fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(crate) fn cache_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|base| base.join("dogit/github-v2"))
}

pub(crate) fn persist(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    let result = || -> std::io::Result<()> {
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(bytes)?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    };
    let _ = result();
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn capture(command: &mut Command, limit: usize, cancel: &AtomicBool) -> Option<Vec<u8>> {
    if cancel.load(Ordering::Acquire) {
        return None;
    }
    let output = hostkit::process::output_cancellable(
        command,
        hostkit::process::CaptureLimits {
            stdout: limit,
            stderr: 4096,
        },
        Duration::from_secs(5),
        &|| cancel.load(Ordering::Acquire),
    )
    .ok()?;
    (output.status.success() && !output.stdout_truncated).then_some(output.stdout)
}

pub(crate) fn fetch(url: &str, limit: usize, cancel: &AtomicBool) -> Option<Vec<u8>> {
    capture(
        Command::new("curl").args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "4",
            "--proto",
            "=https",
            "--user-agent",
            "dogit",
            url,
        ]),
        limit,
        cancel,
    )
}
