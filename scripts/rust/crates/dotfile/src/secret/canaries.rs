use super::vault;
use crate::context::Context;
use aho_corasick::AhoCorasick;
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use zeroize::Zeroize;

pub struct Canary {
    pub label: String,
    pub needle: String,
}
impl Drop for Canary {
    fn drop(&mut self) {
        self.needle.zeroize();
    }
}

pub fn load(context: &Context) -> Result<(Vec<Canary>, Vec<String>), String> {
    let mut values = Vec::new();
    let mut notes = Vec::new();
    let path = context.state.join("canaries");
    if path.is_file() {
        if vault::mode_of(&path)? & 0o077 != 0 {
            notes.push(format!(
                "canaries file is readable beyond you: chmod 600 {}",
                path.display()
            ));
        }
        let text = zeroize::Zeroizing::new(
            String::from_utf8(vault::read_source(&path, super::sops::MAX_SECRET_BYTES)?)
                .map_err(|_| "canaries file is not text")?,
        );
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (label, value) = line
                .split_once('=')
                .map(|(a, b)| (a.trim(), b.trim()))
                .unwrap_or(("private", line));
            if value.chars().count() < 6 {
                notes.push(format!("canary too short to match usefully: {label}"));
                continue;
            }
            values.push(Canary {
                label: label.to_string(),
                needle: value.to_lowercase(),
            });
        }
    }
    let variables = vault::load_variables(context);
    if !variables.note.is_empty() {
        notes.push(variables.note.clone());
    }
    let mut seen: BTreeSet<_> = values.iter().map(|v| v.needle.clone()).collect();
    for (name, value) in &variables.values {
        let lowered = value.to_lowercase();
        if !name.starts_with("open.") && value.chars().count() >= 6 && seen.insert(lowered.clone())
        {
            values.push(Canary {
                label: name.clone(),
                needle: lowered,
            });
        }
    }
    Ok((values, notes))
}

pub fn matcher(values: &[Canary]) -> Result<Option<AhoCorasick>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    AhoCorasick::new(values.iter().map(|v| &v.needle))
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn stream(context: &Context) -> Result<(), String> {
    let (values, notes) = load(context)?;
    for note in notes {
        eprintln!("dotfile: {note}");
    }
    // Regex handles Unicode case folding while preserving original byte ranges.
    let private = values
        .iter()
        .map(|v| {
            regex::RegexBuilder::new(&regex::escape(&v.needle))
                .case_insensitive(true)
                .build()
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = zeroize::Zeroizing::new(Vec::new());
        let count = (&mut input)
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if line.len() > 16 * 1024 * 1024 {
            return Err("redaction input exceeds 16 MiB".into());
        }
        let text: zeroize::Zeroizing<String> = zeroize::Zeroizing::new(
            serde_json::from_slice::<String>(&line)
                .map_err(|_| "redaction input must be a JSON string")?,
        );
        let mut redacted = super::patterns::redact_patterns(&text);
        for pattern in &private {
            redacted = pattern
                .replace_all(&redacted, "[redacted:private]")
                .into_owned();
        }
        serde_json::to_writer(&mut output, &redacted).map_err(|e| e.to_string())?;
        writeln!(output)
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
