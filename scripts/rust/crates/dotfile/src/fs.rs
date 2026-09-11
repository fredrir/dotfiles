use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum Permissions {
    Preserve { new_mode: u32 },
    Exact(u32),
}

fn open_regular(path: &Path) -> Result<Option<(fs::File, fs::Metadata)>, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    match options.open(path) {
        Ok(file) => {
            let metadata = file
                .metadata()
                .map_err(|e| format!("inspect {}: {e}", path.display()))?;
            if !metadata.is_file() {
                return Err(format!("not a regular file: {}", path.display()));
            }
            Ok(Some((file, metadata)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "open {} without following symlinks: {error}",
            path.display()
        )),
    }
}

fn equals(file: &fs::File, metadata: &fs::Metadata, bytes: &[u8]) -> std::io::Result<bool> {
    if metadata.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut reader = file;
    let mut buffer = [0; 8192];
    for wanted in bytes.chunks(buffer.len()) {
        let target = &mut buffer[..wanted.len()];
        match reader.read_exact(target) {
            Ok(()) if target == wanted => {}
            Ok(()) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    match reader.read_exact(&mut buffer[..1]) {
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(true),
        Err(error) => Err(error),
        Ok(()) => Ok(false),
    }
}

pub fn content_matches(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let Some((file, metadata)) = open_regular(path)? else {
        return Ok(false);
    };
    equals(&file, &metadata, bytes).map_err(|e| format!("read {}: {e}", path.display()))
}

/// Replace one file on its own filesystem; unchanged bytes preserve its mtime.
pub fn write_atomic(path: &Path, bytes: &[u8], policy: Permissions) -> Result<bool, String> {
    let existing = open_regular(path)?;
    if let Some((file, metadata)) = &existing
        && equals(file, metadata, bytes).map_err(|e| format!("read {}: {e}", path.display()))?
    {
        #[cfg(unix)]
        if let Permissions::Exact(mode) = policy {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o7777 != mode {
                file.set_permissions(fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("set permissions {}: {e}", path.display()))?;
                file.sync_all()
                    .map_err(|e| format!("sync {}: {e}", path.display()))?;
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let metadata = existing.map(|(_, metadata)| metadata);
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".dotfile-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match policy {
            Permissions::Preserve { new_mode } => new_mode,
            Permissions::Exact(mode) => mode,
        };
        builder.permissions(fs::Permissions::from_mode(mode));
    }
    let mut file = builder
        .tempfile_in(parent)
        .map_err(|e| format!("create temporary file: {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    match policy {
        Permissions::Preserve { .. } => {
            if let Some(metadata) = metadata {
                file.as_file()
                    .set_permissions(metadata.permissions())
                    .map_err(|e| format!("set permissions: {e}"))?;
            }
        }
        Permissions::Exact(mode) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.as_file()
                    .set_permissions(fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("set permissions: {e}"))?;
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
    }
    file.as_file()
        .sync_all()
        .map_err(|e| format!("sync {}: {e}", path.display()))?;
    file.persist(path)
        .map_err(|e| format!("replace {}: {}", path.display(), e.error))?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("sync {}: {e}", parent.display()))?;
    Ok(true)
}

pub fn write_generated(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    write_atomic(path, bytes, Permissions::Preserve { new_mode: 0o644 })
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    write_atomic(path, bytes, Permissions::Exact(0o600))
}

pub mod transaction;

/// Resolve existing ancestors without requiring a not-yet-created leaf to exist.
pub fn resolved(path: &Path) -> Result<PathBuf, String> {
    let mut current = path;
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("inspect {}: {error}", current.display())),
        }
        suffix.push(
            current
                .file_name()
                .ok_or_else(|| format!("cannot resolve {}", path.display()))?
                .to_owned(),
        );
        current = current
            .parent()
            .ok_or_else(|| format!("cannot resolve {}", path.display()))?;
    }
    let mut full = fs::canonicalize(current)
        .map_err(|error| format!("resolve {}: {error}", current.display()))?;
    for part in suffix.into_iter().rev() {
        full.push(part);
    }
    Ok(full)
}
