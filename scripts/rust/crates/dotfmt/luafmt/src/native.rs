use std::fs;
use std::path::Path;

use dotfmt_core::file::replace;

use luafmt::{config::Config, dialect::Dialect, format_with_dialect};

pub fn apply(path: &Path, config: &Config, dialect: Dialect, write: bool) -> Result<bool, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let formatted = format_with_dialect(&text, config, dialect.resolve(config.dialect, path))?;
    if formatted == text {
        return Ok(false);
    }
    if write {
        replace(path, formatted.as_bytes()).map_err(|error| error.to_string())?;
    }
    Ok(true)
}
