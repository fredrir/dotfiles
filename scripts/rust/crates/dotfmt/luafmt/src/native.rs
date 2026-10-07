use std::fs;
use std::io::{self, Write};
use std::path::Path;

use luafmt::{config::Config, dialect::Dialect, format_with_dialect};

pub fn apply(path: &Path, config: &Config, dialect: Dialect, write: bool) -> Result<bool, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let formatted = format_with_dialect(&text, config, dialect.resolve(config.dialect, path))?;
    if formatted == text {
        return Ok(false);
    }
    if write {
        replace(path, &formatted).map_err(|error| error.to_string())?;
    }
    Ok(true)
}

/// Written beside the target and moved over it, so an interrupted run leaves
/// the file it was writing either as it was or as it should be. A rename swaps
/// the inode, so the mode travels with the contents.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let path = &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let permissions = fs::metadata(path)?.permissions();
    let parent = path.parent().filter(|at| !at.as_os_str().is_empty());
    let parent = parent.unwrap_or(Path::new("."));
    // A bounded name also works when the target already reaches NAME_MAX.
    let mut temporary = tempfile::Builder::new()
        .prefix(".luafmt-")
        .tempfile_in(parent)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().set_permissions(permissions)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
