/// One shell word that reads back as `value` in any context.
pub fn word(value: &str) -> String {
    if !value.is_empty() && value.bytes().all(is_plain) && !value.starts_with(['~', '=']) {
        return value.to_string();
    }
    if value.chars().any(char::is_control) {
        return ansi(value);
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `$'...'`, which carries any byte sequence.
pub fn ansi(value: &str) -> String {
    let mut out = String::from("$'");
    for character in value.chars() {
        match character {
            '\\' => out.push_str(r"\\"),
            '\'' => out.push_str(r"\'"),
            '\n' => out.push_str(r"\n"),
            '\t' => out.push_str(r"\t"),
            '\r' => out.push_str(r"\r"),
            c if c.is_control() => out.push_str(&format!(r"\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Characters that a path can contain and still be pasted into any word.
pub fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+' | b'@' | b',' | b':')
}

pub fn is_plain_path(value: &str) -> bool {
    value.starts_with('/') && value.bytes().all(is_plain)
}

#[cfg(test)]
#[path = "../tests/unit/quote_tests.rs"]
mod tests;
