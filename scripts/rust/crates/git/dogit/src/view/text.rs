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

pub(super) fn ansi_line(text: &str) -> Line<'static> {
    parse_ansi_line(text, false, 4)
}

pub(super) fn syntax_line(text: &str, tab_width: usize) -> Line<'static> {
    parse_ansi_line(text, true, tab_width)
}

fn parse_ansi_line(text: &str, preserve_colors: bool, tab_width: usize) -> Line<'static> {
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
            spans.push(Span::styled(std::mem::take(&mut pending), current));
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
                    3 => current = current.add_modifier(Modifier::ITALIC),
                    4 => current = current.add_modifier(Modifier::UNDERLINED),
                    7 if preserve_colors => current = current.add_modifier(Modifier::REVERSED),
                    22 => current = current.remove_modifier(Modifier::BOLD | Modifier::DIM),
                    23 => current = current.remove_modifier(Modifier::ITALIC),
                    24 => current = current.remove_modifier(Modifier::UNDERLINED),
                    27 => current = current.remove_modifier(Modifier::REVERSED),
                    30..=37 | 90..=97 if preserve_colors => {
                        let value = if codes[index] < 90 {
                            codes[index] - 30
                        } else {
                            codes[index] - 90 + 8
                        };
                        current = current.fg(ui_theme::Color::Ansi(value as u8).ratatui());
                    }
                    31 | 91 => current = current.fg(foreground(Role::DiffRemoved)),
                    32 | 92 => current = current.fg(foreground(Role::DiffAdded)),
                    33..=37 | 90 | 93..=97 | 39 => current = current.fg(foreground(Role::Plain)),
                    38 if codes.get(index + 1) == Some(&2) && codes.len() > index + 4 => {
                        current = current.fg(if preserve_colors {
                            ansi_rgb(codes[index + 2], codes[index + 3], codes[index + 4])
                        } else {
                            foreground(inspection_color(
                                codes[index + 2],
                                codes[index + 3],
                                codes[index + 4],
                            ))
                        });
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
                        current = current.fg(if preserve_colors {
                            Color::Indexed(value as u8)
                        } else {
                            foreground(role)
                        });
                        index += 2;
                    }
                    40..=47 | 100..=107 if preserve_colors => {
                        let value = if codes[index] < 100 {
                            codes[index] - 40
                        } else {
                            codes[index] - 100 + 8
                        };
                        current = current.bg(ui_theme::Color::Ansi(value as u8).ratatui());
                    }
                    48 if codes.get(index + 1) == Some(&2) && codes.len() > index + 4 => {
                        if preserve_colors {
                            current = current.bg(ansi_rgb(
                                codes[index + 2],
                                codes[index + 3],
                                codes[index + 4],
                            ));
                        }
                        index += 4;
                    }
                    48 if codes.get(index + 1) == Some(&5) && codes.len() > index + 2 => {
                        if preserve_colors {
                            current = current.bg(Color::Indexed(codes[index + 2].min(255) as u8));
                        }
                        index += 2;
                    }
                    49 => current = current.bg(Color::Reset),
                    _ => {}
                }
                index += 1;
            }
        }
    }
    if !pending.is_empty() {
        spans.push(Span::styled(pending, current));
    }
    let mut expanded = Line::default();
    let mut column = 0;
    visit_graphemes(&Line::from(spans), |symbol, appearance| {
        if symbol == "\t" {
            let count = tab_width.max(1) - column % tab_width.max(1);
            push_styled(&mut expanded, &" ".repeat(count), appearance);
            column += count;
        } else {
            push_styled(&mut expanded, symbol, appearance);
            column += symbol.width();
        }
        true
    });
    expanded
}

fn ansi_rgb(red: u16, green: u16, blue: u16) -> Color {
    ui_theme::Color::Rgb(
        red.min(255) as u8,
        green.min(255) as u8,
        blue.min(255) as u8,
    )
    .at_depth(color_depth())
    .ratatui()
}

pub(super) fn visit_graphemes(line: &Line<'_>, mut visit: impl FnMut(&str, Style) -> bool) {
    let plain: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut span = 0;
    let mut end = line.spans.first().map_or(0, |value| value.content.len());
    let mut offset = 0;
    for segment in plain.split_inclusive('\t') {
        let plain_line = Line::raw(segment.trim_end_matches('\t'));
        let symbols = plain_line
            .styled_graphemes(Style::default())
            .map(|grapheme| grapheme.symbol)
            .chain(segment.ends_with('\t').then_some("\t"));
        for symbol in symbols {
            while offset >= end && span + 1 < line.spans.len() {
                span += 1;
                end += line.spans[span].content.len();
            }
            let appearance = line
                .spans
                .get(span)
                .map_or(line.style, |value| line.style.patch(value.style));
            if !visit(symbol, appearance) {
                return;
            }
            offset += symbol.len();
        }
    }
}

pub(super) fn push_styled(line: &mut Line<'static>, text: &str, appearance: Style) {
    if let Some(last) = line
        .spans
        .last_mut()
        .filter(|span| span.style == appearance)
    {
        last.content.to_mut().push_str(text);
    } else {
        line.spans.push(Span::styled(text.to_string(), appearance));
    }
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
