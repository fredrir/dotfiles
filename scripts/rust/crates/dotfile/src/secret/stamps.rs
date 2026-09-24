//! Proof a secret destination is current without decrypting: ciphertext digests and file metadata only.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::vault::{SecretEntry, SecretKind, identity_path};
use crate::context::Context;

#[derive(Default, Serialize, Deserialize)]
pub struct Stamps {
    destinations: BTreeMap<String, Stamp>,
    #[serde(skip)]
    changed: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    input: String,
    file: File,
}

/// ctime moves on every write, chmod, rename, or replacement, and cannot be set back.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct File {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

/// Everything a destination's content is derived from, shared by every entry.
pub struct Inputs {
    identity: Option<File>,
    variables: Option<[u8; 32]>,
}

impl Inputs {
    pub fn new(context: &Context, entries: &[SecretEntry]) -> Self {
        let templated = entries
            .iter()
            .any(|entry| entry.kind == SecretKind::Template);
        Self {
            identity: file(&identity_path(context)),
            variables: templated
                .then(|| fs::read(context.root.join("vars.enc.yaml")).ok())
                .flatten()
                .map(|bytes| Sha256::digest(bytes).into()),
        }
    }

    /// None when the destination cannot be vouched for without producing it.
    pub fn of(&self, entry: &SecretEntry) -> Option<String> {
        let identity = self.identity?;
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(&identity).ok()?);
        match entry.kind {
            SecretKind::Encrypted => hash.update(b"encrypted\0"),
            SecretKind::Template => {
                hash.update(b"template\0");
                hash.update(self.variables.unwrap_or_default());
            }
            SecretKind::Plain => return None,
        }
        hash.update(fs::read(&entry.source).ok()?);
        Some(format!("{:x}", hash.finalize()))
    }
}

impl Stamps {
    pub fn load(context: &Context) -> Self {
        fs::read(path(context))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn holds(&self, destination: &Path, input: &str) -> bool {
        destination
            .to_str()
            .and_then(|key| self.destinations.get(key))
            .is_some_and(|stamp| stamp.input == input && file(destination) == Some(stamp.file))
    }

    pub fn record(&mut self, destination: &Path, input: Option<&str>) {
        let Some(key) = destination.to_str() else {
            return;
        };
        let stamp = input.zip(file(destination)).map(|(input, file)| Stamp {
            input: input.to_string(),
            file,
        });
        let changed = match stamp {
            Some(stamp) => self.destinations.insert(key.to_string(), stamp.clone()) != Some(stamp),
            None => self.destinations.remove(key).is_some(),
        };
        self.changed |= changed;
    }

    pub fn save(&self, context: &Context) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        crate::fs::write_private(&path(context), &bytes).map(|_| ())
    }
}

fn path(context: &Context) -> PathBuf {
    context.root_config.join("sync/secrets")
}

/// A timestamp without sub-second precision cannot tell two writes in one second apart.
#[cfg(unix)]
fn file(path: &Path) -> Option<File> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.mtime_nsec() == 0 {
        return None;
    }
    Some(File {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        mode: metadata.mode(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

#[cfg(not(unix))]
fn file(_path: &Path) -> Option<File> {
    None
}
