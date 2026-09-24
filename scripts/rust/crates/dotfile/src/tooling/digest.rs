use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const SKIPPED: [&str; 4] = ["target", ".venv", "__pycache__", ".git"];

/// A content digest of every input file, so a rebuild is driven by what the
/// sources say rather than by timestamps a checkout or a touch can move.
pub fn of(inputs: &[PathBuf]) -> Result<String, String> {
    let mut hash = Sha256::new();
    for input in inputs {
        walk(input, input, &mut |relative, path, _| {
            hash.update(relative.as_os_str().as_encoded_bytes());
            hash.update([0]);
            let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if read == 0 {
                    return Ok(());
                }
                hash.update(&buffer[..read]);
            }
        })?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// The last digest of these inputs, reused while no input file's metadata has moved.
pub fn cached(inputs: &[PathBuf], cache: &Path) -> Result<String, String> {
    let fingerprint = fingerprint(inputs)?;
    if let Some(fingerprint) = &fingerprint
        && let Ok(saved) = fs::read_to_string(cache)
        && let Some((saved, digest)) = saved.trim().split_once(' ')
        && saved == fingerprint
    {
        return Ok(digest.to_string());
    }
    let digest = of(inputs)?;
    if let Some(fingerprint) = fingerprint {
        let _ = crate::fs::write_generated(cache, format!("{fingerprint} {digest}\n").as_bytes());
    }
    Ok(digest)
}

/// None when a file's ctime lacks sub-second precision, so two writes in one second look alike.
#[cfg(unix)]
fn fingerprint(inputs: &[PathBuf]) -> Result<Option<String>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut hash = Sha256::new();
    let mut precise = true;
    for input in inputs {
        hash.update(input.as_os_str().as_encoded_bytes());
        hash.update([0]);
        walk(input, input, &mut |relative, _, metadata| {
            precise &= metadata.ctime_nsec() != 0;
            hash.update(relative.as_os_str().as_encoded_bytes());
            hash.update([0]);
            for value in [
                metadata.dev(),
                metadata.ino(),
                metadata.size(),
                u64::from(metadata.mode()),
                metadata.mtime() as u64,
                metadata.mtime_nsec() as u64,
                metadata.ctime() as u64,
                metadata.ctime_nsec() as u64,
            ] {
                hash.update(value.to_le_bytes());
            }
            Ok(())
        })?;
    }
    Ok(precise.then(|| format!("{:x}", hash.finalize())))
}

#[cfg(not(unix))]
fn fingerprint(_inputs: &[PathBuf]) -> Result<Option<String>, String> {
    Ok(None)
}

/// Visits every regular input file in path order, skipping build output and symlinks.
fn walk(
    path: &Path,
    base: &Path,
    visit: &mut dyn FnMut(&Path, &Path, &fs::Metadata) -> Result<(), String>,
) -> Result<(), String> {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SKIPPED.contains(&name))
    {
        return Ok(());
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        let mut children = fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        children.sort();
        for child in children {
            walk(&child, base, visit)?;
        }
        return Ok(());
    }
    visit(path.strip_prefix(base).unwrap_or(path), path, &metadata)
}
