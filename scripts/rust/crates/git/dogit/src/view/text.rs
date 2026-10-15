use super::*;

pub(super) fn safe_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut column = 0;
    for character in text.chars() {
        if character == '\t' {
            let spaces = 4 - column % 4;
            result.push_str(&" ".repeat(spaces));
            column += spaces;
        } else if !character.is_control() {
            result.push(character);
            column += character.width().unwrap_or(0);
        }
    }
    result
}

pub(super) fn safe_text_multiline(text: &str) -> String {
    text.split('\n')
        .map(safe_text)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn elide_middle(text: &str, width: usize) -> String {
    let clean = safe_text(text);
    if clean.width() <= width {
        return clean;
    }
    if width < 4 {
        return window(&clean, 0, width);
    }
    let left = (width - 1) / 3;
    let right = width - 1 - left;
    format!(
        "{}…{}",
        window(&clean, 0, left),
        window(&clean, clean.width().saturating_sub(right), right)
    )
}

pub(super) fn elide_end(text: &str, width: usize) -> String {
    let clean = safe_text(text);
    if clean.width() <= width {
        clean
    } else if width == 0 {
        String::new()
    } else {
        format!("{}…", window(&clean, 0, width - 1))
    }
}

pub(super) fn window(text: &str, offset: usize, width: usize) -> String {
    let mut output = String::new();
    let mut column = 0;
    for character in text.chars() {
        let length = character.width().unwrap_or(0);
        if column + length > offset + width {
            break;
        }
        if column >= offset {
            output.push(character);
        }
        column += length;
    }
    output
}

/// Retains semantic foreground and emphasis, never external colors or backgrounds.
pub(super) fn ansi_line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current = style(Role::Plain);
    let mut pending = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            if !character.is_control() || character == '\t' {
                pending.push(character);
            }
            continue;
        }
        if !pending.is_empty() {
            spans.push(Span::styled(safe_text(&pending), current));
            pending.clear();
        }
        match characters.next() {
            Some('[') => {}
            Some(']' | 'P' | '^' | '_') => {
                while let Some(next) = characters.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && characters.peek() == Some(&'\\') {
                        characters.next();
                        break;
                    }
                }
                continue;
            }
            Some('(' | ')') => {
                characters.next();
                continue;
            }
            _ => continue,
        }
        let mut parameters = String::new();
        let mut end = None;
        for next in characters.by_ref() {
            if ('@'..='~').contains(&next) {
                end = Some(next);
                break;
            }
            if parameters.len() < 128 {
                parameters.push(next);
            }
        }
        if end == Some('m') {
            let codes = parameters
                .split(';')
                .map(|value| value.parse::<u16>().unwrap_or(0))
                .collect::<Vec<_>>();
            let mut index = 0;
            while index < codes.len() {
                match codes[index] {
                    0 => current = style(Role::Plain),
                    1 => current = current.add_modifier(Modifier::BOLD),
                    3 => current = current.add_modifier(Modifier::ITALIC),
                    4 => current = current.add_modifier(Modifier::UNDERLINED),
                    22 => current = current.remove_modifier(Modifier::BOLD | Modifier::DIM),
                    23 => current = current.remove_modifier(Modifier::ITALIC),
                    24 => current = current.remove_modifier(Modifier::UNDERLINED),
                    31 | 91 => current = current.fg(foreground(Role::DiffRemoved)),
                    32 | 92 => current = current.fg(foreground(Role::DiffAdded)),
                    33..=37 | 90 | 93..=97 | 39 => current = current.fg(foreground(Role::Plain)),
                    38 if codes.get(index + 1) == Some(&2) && codes.len() > index + 4 => {
                        current = current.fg(foreground(inspection_color(
                            codes[index + 2],
                            codes[index + 3],
                            codes[index + 4],
                        )));
                        index += 4;
                    }
                    38 if codes.get(index + 1) == Some(&5) && codes.len() > index + 2 => {
                        let value = codes[index + 2].min(255);
                        let role = match value {
                            1 | 9 => Role::DiffRemoved,
                            2 | 10 => Role::DiffAdded,
                            16..=231 => {
                                let cube = [0, 95, 135, 175, 215, 255];
                                let color = usize::from(value - 16);
                                inspection_color(
                                    cube[color / 36],
                                    cube[(color / 6) % 6],
                                    cube[color % 6],
                                )
                            }
                            _ => Role::Plain,
                        };
                        current = current.fg(foreground(role));
                        index += 2;
                    }
                    48 if codes.get(index + 1) == Some(&2) => index = (index + 4).min(codes.len()),
                    48 if codes.get(index + 1) == Some(&5) => index = (index + 2).min(codes.len()),
                    _ => {}
                }
                index += 1;
            }
        }
    }
    if !pending.is_empty() {
        spans.push(Span::styled(safe_text(&pending), current));
    }
    Line::from(spans)
}

fn inspection_color(red: u16, green: u16, blue: u16) -> Role {
    let (red, green, blue) = (red.min(255), green.min(255), blue.min(255));
    if red > green.saturating_add(25) && red > blue.saturating_add(25) {
        Role::DiffRemoved
    } else if green > red.saturating_add(25) && green > blue.saturating_add(10) {
        Role::DiffAdded
    } else {
        Role::Plain
    }
}
