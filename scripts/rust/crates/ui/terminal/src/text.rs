use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn sanitize(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control()
            || matches!(character,
            '\u{061c}' | '\u{200b}' | '\u{200e}'..='\u{200f}' |
            '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
        {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

pub fn width(text: &str) -> usize {
    if !text.contains('\u{1b}') {
        return UnicodeWidthStr::width(text);
    }
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.next() == Some('[') {
                for control in chars.by_ref() {
                    if ('@'..='~').contains(&control) {
                        break;
                    }
                }
            }
        } else {
            plain.push(character);
        }
    }
    UnicodeWidthStr::width(plain.as_str())
}

pub fn fit(text: &str, limit: usize) -> String {
    truncate_back(&sanitize(text), limit)
}

pub fn pad_right(text: &str, cells: usize) -> String {
    format!("{text}{}", " ".repeat(cells.saturating_sub(width(text))))
}

/// Wrap a plain line by terminal cells; escape controls before measuring.
pub fn wrap(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return vec![String::new()];
    }
    let safe = sanitize(text);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in safe.split(' ') {
        let cells = width(word);
        if used > 0 && used + 1 + cells <= limit {
            line.push(' ');
            line.push_str(word);
            used += 1 + cells;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        for character in word.chars() {
            let cells = UnicodeWidthChar::width(character).unwrap_or(0);
            if used + cells > limit && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if cells > limit {
                line.push('…');
                used += 1;
            } else {
                line.push(character);
                used += cells;
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn truncate_back(text: &str, limit: usize) -> String {
    if UnicodeWidthStr::width(text) <= limit {
        return text.to_owned();
    }
    if limit == 0 {
        return String::new();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cells = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + cells > limit - 1 {
            break;
        }
        output.push(character);
        used += cells;
    }
    output.push('…');
    output
}

pub fn truncate_front(text: &str, limit: usize) -> String {
    if UnicodeWidthStr::width(text) <= limit {
        return text.to_owned();
    }
    if limit == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let cells = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + cells > limit - 1 {
            break;
        }
        used += cells;
        start = index;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
#[path = "../tests/unit/text_tests.rs"]
mod tests;
