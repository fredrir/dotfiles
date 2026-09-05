pub fn truncate_front(text: &str, room: usize) -> String {
    let length = text.chars().count();
    if length <= room {
        return text.to_string();
    }
    let tail: String = text.chars().skip(length - room.saturating_sub(1)).collect();
    format!("\u{2026}{tail}")
}

pub fn truncate_back(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    if limit <= 1 {
        return "\u{2026}".repeat(limit);
    }
    let kept: String = text.chars().take(limit - 1).collect();
    format!("{kept}\u{2026}")
}

pub fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", plural(count, one, many))
}

#[cfg(test)]
#[path = "../tests/unit/text_tests.rs"]
mod tests;
