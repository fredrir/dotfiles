use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{config::Config, format};
use mdfmt::dialect::Dialect;

pub fn apply(path: &Path, config: &Config, dialect: Dialect, write: bool) -> Result<bool, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut config = config.clone();
    config.dialect = dialect.resolve(config.dialect, path);
    let formatted = format(&text, &config)?;
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
    let (mut file, temporary) = sibling(path)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::set_permissions(&temporary, permissions))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        fs::remove_file(&temporary).ok();
    }
    written
}

fn sibling(path: &Path) -> io::Result<(File, PathBuf)> {
    let parent = path.parent().filter(|at| !at.as_os_str().is_empty());
    let parent = parent.unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().display().to_string();
    let mut attempt = 0;
    loop {
        let temporary = parent.join(format!(".{name}.mdfmt-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
