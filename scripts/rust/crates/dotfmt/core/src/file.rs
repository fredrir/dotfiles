use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Done {
    Unchanged,
    Changed,
}

pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let path = &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let permissions = fs::metadata(path)?.permissions();
    let parent = path.parent().filter(|at| !at.as_os_str().is_empty());
    let parent = parent.unwrap_or(Path::new("."));
    // A bounded name also works when the target already reaches NAME_MAX.
    let mut temporary = tempfile::Builder::new()
        .prefix(".dotfmt-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().set_permissions(permissions)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
