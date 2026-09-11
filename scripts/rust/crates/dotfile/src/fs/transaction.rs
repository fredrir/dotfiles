//! Durable, checked rollback for filesystem mutations, using atomic renames.
use crate::context::Context;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PREFIX: &str = ".dotfile-transaction-";

fn temporary_directory(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    let temporary = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .map_err(|error| format!("create private transaction directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protect transaction directory: {error}"))?;
    }
    Ok(temporary.keep())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    kind: u8,
    // Directory contents legitimately change during a transaction. File data must not.
    size: u64,
    modified: i128,
}
impl Identity {
    fn read(path: &Path) -> Result<Self, String> {
        let metadata =
            fs::symlink_metadata(path).map_err(|e| format!("inspect {}: {e}", path.display()))?;
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (device, inode) = (0, 0);
        let directory = metadata.is_dir() && !metadata.file_type().is_symlink();
        let kind = if directory {
            1
        } else if metadata.file_type().is_symlink() {
            2
        } else if metadata.is_file() {
            3
        } else {
            return Err(format!("unsupported file type: {}", path.display()));
        };
        let modified = if directory {
            0
        } else {
            metadata
                .modified()
                .map_err(|e| e.to_string())?
                .duration_since(std::time::UNIX_EPOCH)
                .map_or_else(
                    |e| -(e.duration().as_nanos() as i128),
                    |value| value.as_nanos() as i128,
                )
        };
        Ok(Self {
            device,
            inode,
            kind,
            size: if directory { 0 } else { metadata.len() },
            modified,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Undo {
    source: PathBuf,
    destination: PathBuf,
    identity: Identity,
    empty_directory: bool,
    tree_digest: Option<String>,
    original: Option<Identity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Resource {
    path: PathBuf,
    identity: Identity,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    version: u8,
    root: PathBuf,
    resources: Vec<Resource>,
    undo: Vec<Undo>,
    committed: bool,
}

pub struct Transaction {
    directory: PathBuf,
    journal: Journal,
    finished: bool,
}

fn normalized(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let name = absolute
        .file_name()
        .ok_or_else(|| format!("invalid transaction path: {}", path.display()))?;
    Ok(crate::fs::resolved(absolute.parent().ok_or("path has no parent")?)?.join(name))
}
fn sync_directory(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("sync {}: {e}", path.display()))
}
fn private_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("unsafe transaction directory: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(format!(
                "transaction directory has unsafe ownership or permissions: {}",
                path.display()
            ));
        }
    }
    Ok(())
}
fn stable_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("transaction path has no parent")?;
    if fs::canonicalize(parent).map_err(|e| format!("inspect {}: {e}", parent.display()))? != parent
    {
        return Err(format!(
            "transaction parent changed or became a symlink: {}",
            parent.display()
        ));
    }
    Ok(())
}
fn save(directory: &Path, journal: &Journal) -> Result<(), String> {
    let bytes = serde_json::to_vec(journal).map_err(|e| e.to_string())?;
    crate::fs::write_private(&directory.join("journal.json"), &bytes)?;
    Ok(())
}

fn tree_digest(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    fn node(path: &Path, hash: &mut Sha256) -> Result<(), String> {
        let before = Identity::read(path)?;
        hash.update([before.kind]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            hash.update(
                fs::symlink_metadata(path)
                    .map_err(|e| e.to_string())?
                    .mode()
                    .to_le_bytes(),
            );
        }
        match before.kind {
            1 => {
                let mut entries = fs::read_dir(path)
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                entries.sort_by_key(|entry| entry.file_name());
                hash.update((entries.len() as u64).to_le_bytes());
                for entry in entries {
                    let name = entry.file_name();
                    let bytes = name.as_encoded_bytes();
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes);
                    node(&entry.path(), hash)?;
                }
            }
            2 => {
                let target = fs::read_link(path).map_err(|e| e.to_string())?;
                let bytes = target.as_os_str().as_encoded_bytes();
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            _ => {
                let mut options = fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let mut file = options.open(path).map_err(|e| e.to_string())?;
                hash.update(before.size.to_le_bytes());
                let mut buffer = [0_u8; 65_536];
                loop {
                    let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
            }
        }
        if Identity::read(path)? != before {
            return Err(format!(
                "file changed during recovery inspection: {}",
                path.display()
            ));
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    node(path, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_tree(path: &Path, expected: Option<&str>) -> Result<(), String> {
    if let Some(expected) = expected
        && tree_digest(path)? != expected
    {
        return Err(format!(
            "recovery preserves changed cross-filesystem contents: {}",
            path.display()
        ));
    }
    Ok(())
}

fn rollback_one(undo: &Undo) -> Result<(), String> {
    stable_parent(&undo.source)?;
    stable_parent(&undo.destination)?;
    if let Some(original) = &undo.original {
        let current = Identity::read(&undo.source)?;
        if &current == original {
            if fs::symlink_metadata(&undo.destination).is_ok()
                && Identity::read(&undo.destination)? != *original
            {
                return Err(format!(
                    "recovery backup changed: {}",
                    undo.destination.display()
                ));
            }
            return Ok(());
        }
        if current != undo.identity || Identity::read(&undo.destination)? != *original {
            return Err(format!(
                "recovery refuses a changed occupant: {}",
                undo.source.display()
            ));
        }
        fs::rename(&undo.destination, &undo.source)
            .map_err(|error| format!("restore {}: {error}", undo.source.display()))?;
        sync_directory(undo.source.parent().unwrap())?;
        return sync_directory(undo.destination.parent().unwrap());
    }
    let source = fs::symlink_metadata(&undo.source);
    let destination = fs::symlink_metadata(&undo.destination);
    match (source, destination) {
        (Err(error), Ok(_)) if error.kind() == std::io::ErrorKind::NotFound => {
            if Identity::read(&undo.destination)? != undo.identity {
                return Err(format!(
                    "recovery refuses a changed occupant: {}",
                    undo.destination.display()
                ));
            }
            verify_tree(&undo.destination, undo.tree_digest.as_deref())?;
            Ok(()) // Intent was not applied, or an earlier recovery already reversed it.
        }
        (Ok(_), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            if Identity::read(&undo.source)? != undo.identity {
                return Err(format!(
                    "recovery refuses a changed occupant: {}",
                    undo.source.display()
                ));
            }
            verify_tree(&undo.source, undo.tree_digest.as_deref())?;
            if undo.empty_directory
                && fs::read_dir(&undo.source)
                    .map_err(|e| e.to_string())?
                    .next()
                    .is_some()
            {
                return Err(format!(
                    "recovery preserves newly added contents: {}",
                    undo.source.display()
                ));
            }
            fs::rename(&undo.source, &undo.destination)
                .map_err(|e| format!("restore {}: {e}", undo.destination.display()))?;
            sync_directory(undo.source.parent().unwrap())?;
            sync_directory(undo.destination.parent().unwrap())
        }
        _ => Err(format!(
            "recovery refuses ambiguous occupants: {} and {}",
            undo.source.display(),
            undo.destination.display()
        )),
    }
}
fn rollback(directory: &Path, journal: &mut Journal) -> Result<(), String> {
    while let Some(undo) = journal.undo.last() {
        rollback_one(undo)?;
        journal.undo.pop();
        save(directory, journal)?;
    }
    Ok(())
}
fn cleanup(directory: &Path, journal: &Journal) -> Result<(), String> {
    for resource in &journal.resources {
        if !resource.path.exists() {
            continue;
        }
        private_directory(&resource.path)?;
        stable_parent(&resource.path)?;
        if Identity::read(&resource.path)? != resource.identity {
            return Err(format!(
                "recovery resource changed: {}",
                resource.path.display()
            ));
        }
        fs::remove_dir_all(&resource.path)
            .map_err(|e| format!("clean recovery files {}: {e}", resource.path.display()))?;
        sync_directory(resource.path.parent().unwrap())?;
    }
    fs::remove_dir_all(directory)
        .map_err(|e| format!("clean transaction {}: {e}", directory.display()))?;
    sync_directory(directory.parent().unwrap())
}

/// Run under the repository mutation lock, before inspecting source/destination paths.
pub fn recover(context: &Context) -> Result<(), String> {
    recover_root(&context.root)
}

fn recover_root(root: &Path) -> Result<(), String> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut directories = fs::read_dir(&root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(PREFIX))
        })
        .collect::<Vec<_>>();
    directories.sort();
    for directory in directories {
        private_directory(&directory)?;
        let path = directory.join("journal.json");
        if !path.exists()
            && fs::read_dir(&directory)
                .map_err(|e| e.to_string())?
                .next()
                .is_none()
        {
            fs::remove_dir(&directory).map_err(|e| e.to_string())?;
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("recovery journal {}: {e}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!("unsafe recovery journal: {}", path.display()));
        }
        let mut journal: Journal =
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("recovery journal {}: {e}", path.display()))?;
        if journal.version != 1 || journal.root != root {
            return Err(format!(
                "recovery journal belongs to a different repository: {}",
                path.display()
            ));
        }
        for resource in &journal.resources {
            if !resource.path.is_absolute()
                || !resource
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".dotfile-recovery-"))
            {
                return Err("invalid recovery resource path".into());
            }
        }
        for undo in &journal.undo {
            if !undo.source.is_absolute() || !undo.destination.is_absolute() {
                return Err("recovery journal paths must be absolute".into());
            }
        }
        if !journal.committed {
            rollback(&directory, &mut journal).map_err(|error| {
                format!(
                    "{error}; recovery files retained at {}",
                    directory.display()
                )
            })?;
        }
        cleanup(&directory, &journal)?;
    }
    Ok(())
}

impl Transaction {
    pub fn new(context: &Context) -> Result<Self, String> {
        Self::for_root(&context.root)
    }

    pub fn for_root(root: &Path) -> Result<Self, String> {
        recover_root(root)?;
        let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        let directory = temporary_directory(&root, PREFIX)?;
        let journal = Journal {
            version: 1,
            root,
            resources: vec![],
            undo: vec![],
            committed: false,
        };
        save(&directory, &journal)?;
        sync_directory(directory.parent().unwrap())?;
        Ok(Self {
            directory,
            journal,
            finished: false,
        })
    }
    fn staging(&mut self, parent: &Path) -> Result<PathBuf, String> {
        let parent = if Identity::read(parent)?.device == Identity::read(&self.directory)?.device {
            self.directory.as_path()
        } else {
            parent
        };
        let path = temporary_directory(parent, ".dotfile-recovery-")?;
        self.journal.resources.push(Resource {
            identity: Identity::read(&path)?,
            path: path.clone(),
        });
        save(&self.directory, &self.journal)?;
        sync_directory(parent)?;
        Ok(path)
    }
    fn rename(
        &mut self,
        source: &Path,
        destination: &Path,
        empty_directory: bool,
    ) -> Result<(), String> {
        self.rename_checked(source, destination, empty_directory, None)
    }
    fn rename_checked(
        &mut self,
        source: &Path,
        destination: &Path,
        empty_directory: bool,
        tree_digest: Option<String>,
    ) -> Result<(), String> {
        let source = normalized(source)?;
        let destination = normalized(destination)?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(format!("destination exists: {}", destination.display()));
        }
        self.journal.undo.push(Undo {
            identity: Identity::read(&source)?,
            source: destination.clone(),
            destination: source.clone(),
            empty_directory,
            tree_digest,
            original: None,
        });
        save(&self.directory, &self.journal)?;
        fs::rename(&source, &destination).map_err(|e| {
            format!(
                "move {} -> {}: {e}",
                source.display(),
                destination.display()
            )
        })?;
        sync_directory(source.parent().unwrap())?;
        sync_directory(destination.parent().unwrap())
    }
    fn replace(&mut self, staged: &Path, destination: &Path) -> Result<(), String> {
        let staged = normalized(staged)?;
        let destination = normalized(destination)?;
        let original = Identity::read(&destination)?;
        if original.kind != 3 {
            return Err(format!(
                "refusing to replace non-file {}",
                destination.display()
            ));
        }
        let backup = self
            .staging(destination.parent().unwrap())?
            .join("original");
        fs::hard_link(&destination, &backup).map_err(|error| {
            format!(
                "retain atomic backup for {}: {error}",
                destination.display()
            )
        })?;
        sync_directory(backup.parent().unwrap())?;
        if Identity::read(&destination)? != original || Identity::read(&backup)? != original {
            return Err(format!(
                "file changed while preparing replacement: {}",
                destination.display()
            ));
        }
        self.journal.undo.push(Undo {
            source: destination.clone(),
            destination: backup,
            identity: Identity::read(&staged)?,
            original: Some(original),
            empty_directory: false,
            tree_digest: None,
        });
        save(&self.directory, &self.journal)?;
        fs::rename(&staged, &destination)
            .map_err(|error| format!("replace {}: {error}", destination.display()))?;
        sync_directory(staged.parent().unwrap())?;
        sync_directory(destination.parent().unwrap())
    }
    pub fn mkdir(&mut self, path: &Path) -> Result<(), String> {
        if path.is_dir() {
            return Ok(());
        }
        if fs::symlink_metadata(path).is_ok() {
            return Err(format!("not a directory: {}", path.display()));
        }
        let path = normalized(path)?;
        let parent = path.parent().ok_or("directory has no parent")?;
        self.mkdir(parent)?;
        let staging = self.staging(parent)?;
        let staged = staging.join("directory");
        fs::create_dir(&staged).map_err(|e| e.to_string())?;
        sync_directory(&staging)?;
        self.rename(&staged, &path, true)
    }
    pub fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let path = normalized(path)?;
        self.mkdir(path.parent().ok_or("file has no parent")?)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                Some(metadata)
            }
            Ok(_) => return Err(format!("refusing to overwrite non-file {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("inspect {}: {e}", path.display())),
        };
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.len() == bytes.len() as u64)
            && crate::fs::content_matches(&path, bytes)?
        {
            return Ok(());
        }
        let staging = self.staging(path.parent().unwrap())?;
        let staged = staging.join("file");
        crate::fs::write_generated(&staged, bytes)?;
        if let Some(metadata) = metadata {
            fs::set_permissions(&staged, metadata.permissions()).map_err(|e| e.to_string())?;
            fs::File::open(&staged)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            return self.replace(&staged, &path);
        }
        self.rename(&staged, &path, false)
    }
    pub fn move_node(&mut self, source: &Path, destination: &Path) -> Result<(), String> {
        let source = normalized(source)?;
        let destination = normalized(destination)?;
        self.mkdir(destination.parent().ok_or("destination has no parent")?)?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(format!("destination exists: {}", destination.display()));
        }
        if Identity::read(&source)?.device == Identity::read(destination.parent().unwrap())?.device
        {
            return self.rename(&source, &destination, false);
        }
        let staging = self.staging(destination.parent().unwrap())?;
        let staged = staging.join("copy");
        let before = tree_digest(&source)?;
        copy_node(&source, &staged)?;
        if before != tree_digest(&source)? || before != tree_digest(&staged)? {
            return Err(format!(
                "source changed during cross-filesystem copy: {}",
                source.display()
            ));
        }
        sync_directory(&staging)?;
        self.discard(&source)?;
        self.rename_checked(&staged, &destination, false, Some(before))
    }
    pub fn discard(&mut self, path: &Path) -> Result<(), String> {
        let path = normalized(path)?;
        let staging = self.staging(path.parent().ok_or("path has no parent")?)?;
        self.rename(&path, &staging.join("original"), false)
    }
    pub fn symlink(&mut self, target: &Path, link: &Path) -> Result<(), String> {
        let link = normalized(link)?;
        self.mkdir(link.parent().ok_or("link has no parent")?)?;
        let staging = self.staging(link.parent().unwrap())?;
        let staged = staging.join("link");
        symlink(target, &staged)?;
        sync_directory(&staging)?;
        self.rename(&staged, &link, false)
    }
    pub fn remove_empty(&mut self, path: &Path) -> Result<(), String> {
        if fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .next()
            .is_none()
        {
            self.discard(path)?;
        }
        Ok(())
    }
    pub fn stage(&mut self, context: &Context, paths: &[PathBuf]) -> Result<(), String> {
        // A removed package can contain newly adopted, still-untracked files.
        // Git rejects a nonexistent pathspec unless the index knows that path.
        let missing = paths
            .iter()
            .filter(|path| !context.root.join(path).exists())
            .collect::<Vec<_>>();
        let tracked = if missing.is_empty() {
            Vec::new()
        } else {
            let mut command = context.command("git");
            command
                .arg("-C")
                .arg(&context.root)
                .args(["--literal-pathspecs", "ls-files", "--cached", "-z", "--"])
                .args(&missing);
            let output = crate::process::output(
                &mut command,
                hostkit::process::CaptureLimits::default(),
                Duration::from_secs(15),
            )
            .map_err(|error| format!("inspect Git staging paths: {error}"))?;
            if !output.status.success() {
                return Err("cannot inspect Git staging paths".into());
            }
            output.stdout
        };
        let paths = paths
            .iter()
            .filter(|path| {
                if context.root.join(path).exists() {
                    return true;
                }
                let relative = path
                    .strip_prefix(&context.root)
                    .unwrap_or(path)
                    .as_os_str()
                    .as_encoded_bytes();
                tracked.split(|byte| *byte == 0).any(|tracked| {
                    tracked == relative
                        || tracked
                            .strip_prefix(relative)
                            .is_some_and(|suffix| suffix.starts_with(b"/"))
                })
            })
            .collect::<Vec<_>>();
        let mut command = context.command("git");
        command
            .arg("-C")
            .arg(&context.root)
            .args(["rev-parse", "--git-path", "index"]);
        let output = crate::process::output(
            &mut command,
            hostkit::process::CaptureLimits::default(),
            Duration::from_secs(15),
        )
        .map_err(|error| format!("locate Git index: {error}"))?;
        if !output.status.success() {
            return Err("cannot locate the repository Git index".into());
        }
        let index = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let index = if index.is_absolute() {
            index
        } else {
            context.root.join(index)
        };
        let index = normalized(&index)?;
        let before = match fs::read(&index) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("read Git index: {error}")),
        };
        if fs::symlink_metadata(&index).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err("refusing symlink Git index".into());
        }
        let staging = self.staging(index.parent().ok_or("Git index has no parent")?)?;
        let staged = staging.join("index");
        if before.is_some() {
            copy_node(&index, &staged)?;
        }
        let mut command = context.command("git");
        command
            .arg("-C")
            .arg(&context.root)
            .env("GIT_INDEX_FILE", &staged)
            .args(["--literal-pathspecs", "add", "-A", "--"])
            .args(paths);
        let output = crate::process::output(
            &mut command,
            hostkit::process::CaptureLimits::default(),
            Duration::from_secs(30),
        )
        .map_err(|error| format!("stage repository changes: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Git staging failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        if before == fs::read(&index).ok() {
            if staged.exists() {
                fs::File::open(&staged)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| format!("sync staged Git index: {error}"))?;
                if before.is_some() {
                    self.replace(&staged, &index)?;
                } else {
                    self.rename(&staged, &index, false)?;
                }
            }
        } else {
            return Err("Git index changed while preparing staging; transaction cancelled".into());
        }
        Ok(())
    }
    pub fn commit(mut self) -> Result<(), String> {
        self.journal.committed = true;
        if let Err(error) = save(&self.directory, &self.journal) {
            // Atomic replacement may have succeeded before its directory sync failed.
            // Let recovery read the durable marker before deciding which direction to go.
            self.finished = true;
            return Err(format!(
                "commit durability uncertain: {error}; recovery files retained at {}",
                self.directory.display()
            ));
        }
        self.finished = true;
        cleanup(&self.directory, &self.journal)
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let result = rollback(&self.directory, &mut self.journal)
            .and_then(|()| cleanup(&self.directory, &self.journal));
        if let Err(error) = result {
            eprintln!(
                "dotfile: {error}; recovery files retained at {}",
                self.directory.display()
            );
        }
    }
}

pub fn remove_node(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| format!("remove {}: {error}", path.display()))
}

fn copy_node(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return symlink(
            &fs::read_link(source).map_err(|error| error.to_string())?,
            destination,
        );
    }
    if metadata.is_dir() {
        fs::create_dir(destination).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_node(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())
            .map_err(|error| error.to_string())?;
        sync_directory(destination)?;
    } else if metadata.is_file() {
        fs::copy(source, destination).map_err(|error| error.to_string())?;
        fs::File::open(destination)
            .and_then(|file| {
                file.set_times(fs::FileTimes::new().set_modified(metadata.modified()?))
                    .and_then(|()| file.sync_all())
            })
            .map_err(|error| error.to_string())?;
    } else {
        return Err(format!("unsupported file type: {}", source.display()));
    }
    Ok(())
}

fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|error| format!("link {}: {error}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("configuration symlinks require Unix".into())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/transaction_tests.rs"]
mod tests;
