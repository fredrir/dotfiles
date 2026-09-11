use std::fs;
use std::path::Path;

pub fn render_block(name: &str, keys: &[(String, String)]) -> String {
    let width = keys.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let mut block = format!("{name} {{\n");
    for (key, value) in keys {
        block.push_str(&format!("  {key:<width$}  = {value}\n"));
    }
    block.push('}');
    block
}

pub fn append(path: &Path, block: &str) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let trimmed = existing.trim_end();
    let content = if trimmed.is_empty() {
        block.to_string()
    } else {
        format!("{trimmed}\n\n{block}")
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "../../tests/unit/stress/log_tests.rs"]
mod tests;
