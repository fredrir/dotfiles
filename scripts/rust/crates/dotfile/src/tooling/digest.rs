use std::fs;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

const SKIPPED: [&str; 4] = ["target", ".venv", "__pycache__", ".git"];

/// A content digest of every input file, so a rebuild is driven by what the
/// sources say rather than by timestamps a checkout or a touch can move.
pub fn of(inputs: &[std::path::PathBuf]) -> Result<String, String> {
    let mut hash = Sha256::new();
    for input in inputs {
        node(input, input, &mut hash)?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn node(path: &Path, base: &Path, hash: &mut Sha256) -> Result<(), String> {
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
            node(&child, base, hash)?;
        }
        return Ok(());
    }
    let relative = path.strip_prefix(base).unwrap_or(path);
    hash.update(relative.as_os_str().as_encoded_bytes());
    hash.update([0]);
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(())
}
