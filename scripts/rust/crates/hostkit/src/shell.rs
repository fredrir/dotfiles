use std::path::Path;

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub fn quote_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(quote)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
#[path = "../tests/unit/shell_tests.rs"]
mod tests;
