use std::fs;
use std::path::Path;

use dotfmt_core::file::replace;

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
        replace(path, formatted.as_bytes()).map_err(|error| error.to_string())?;
    }
    Ok(true)
}
