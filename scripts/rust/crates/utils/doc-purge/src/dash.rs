use crate::lang::Dialect;
use crate::scan::{Bail, Out, char_width, identifier_byte, line_end, starts_with};

const SYMBOLS: &[u8] = b"!#$%&*+./<=>?@\\^|-~:";

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    match dialect {
        Dialect::Lua => lua(source),
        Dialect::Sql => sql(source),
        Dialect::Haskell => haskell(source),
        _ => Ok(Out::default()),
    }
}

fn long_level(source: &[u8], at: usize) -> Option<usize> {
    if source.get(at) != Some(&b'[') {
        return None;
    }
    let mut cursor = at + 1;
    let mut level = 0usize;
    while source.get(cursor) == Some(&b'=') {
        level += 1;
        cursor += 1;
    }
    (source.get(cursor) == Some(&b'[')).then_some(level)
}

fn long_close(source: &[u8], from: usize, level: usize) -> Option<usize> {
    let mut closing = Vec::with_capacity(level + 2);
    closing.push(b']');
    closing.extend(std::iter::repeat_n(b'=', level));
    closing.push(b']');
    let mut cursor = from;
    while cursor < source.len() {
        if starts_with(source, cursor, &closing) {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn lua(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        let byte = source[at];
        if starts_with(source, at, b"--") {
            let start = at;
            if let Some(level) = long_level(source, at + 2) {
                let body = at + 2 + level + 2;
                let Some(close) = long_close(source, body, level) else {
                    return Err(Bail("unterminated long comment"));
                };
                let end = close + level + 2;
                out.comment(start..end, body..close);
                at = end;
                continue;
            }
            let end = line_end(source, at);
            out.comment(start..end, start + 2..end);
            at = end;
            continue;
        }
        match byte {
            b'[' => {
                if let Some(level) = long_level(source, at) {
                    let body = at + level + 2;
                    let Some(close) = long_close(source, body, level) else {
                        return Err(Bail("unterminated long string"));
                    };
                    out.text(body..close, false, level > 0, 0);
                    at = close + level + 2;
                    continue;
                }
                at += 1;
            }
            b'\'' | b'"' => {
                let start = at + 1;
                let mut cursor = start;
                loop {
                    if cursor >= source.len() || source[cursor] == b'\n' {
                        return Err(Bail("unterminated string"));
                    }
                    if source[cursor] == b'\\' {
                        cursor += 2;
                        continue;
                    }
                    if source[cursor] == byte {
                        out.text(start..cursor, true, true, byte);
                        at = cursor + 1;
                        break;
                    }
                    cursor += char_width(source, cursor);
                }
            }
            _ => at += char_width(source, at),
        }
    }
    Ok(out)
}

fn sql(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        let byte = source[at];
        if starts_with(source, at, b"--") {
            let end = line_end(source, at);
            out.comment(at..end, at + 2..end);
            at = end;
            continue;
        }
        if starts_with(source, at, b"/*") {
            let start = at;
            let mut cursor = at + 2;
            let mut depth = 1usize;
            while cursor < source.len() {
                if starts_with(source, cursor, b"/*") {
                    depth += 1;
                    cursor += 2;
                    continue;
                }
                if starts_with(source, cursor, b"*/") {
                    cursor += 2;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                cursor += 1;
            }
            if depth > 0 {
                return Err(Bail("unterminated block comment"));
            }
            out.comment(start..cursor, start + 2..cursor - 2);
            at = cursor;
            continue;
        }
        match byte {
            b'\'' => {
                let start = at + 1;
                let mut cursor = start;
                loop {
                    if cursor >= source.len() {
                        return Err(Bail("unterminated string"));
                    }
                    if source[cursor] == b'\'' {
                        if source.get(cursor + 1) == Some(&b'\'') {
                            cursor += 2;
                            continue;
                        }
                        out.text(start..cursor, false, true, b'\'');
                        at = cursor + 1;
                        break;
                    }
                    cursor += char_width(source, cursor);
                }
            }
            b'"' => {
                let mut cursor = at + 1;
                while cursor < source.len() && source[cursor] != b'"' {
                    cursor += char_width(source, cursor);
                }
                if cursor >= source.len() {
                    return Err(Bail("unterminated quoted name"));
                }
                at = cursor + 1;
            }
            b'$' => {
                let mut cursor = at + 1;
                while cursor < source.len() && identifier_byte(source[cursor]) {
                    cursor += 1;
                }
                if source.get(cursor) != Some(&b'$') {
                    at += 1;
                    continue;
                }
                let tag = &source[at..cursor + 1];
                let start = cursor + 1;
                let mut probe = start;
                let close = loop {
                    if probe >= source.len() {
                        return Err(Bail("unterminated dollar quote"));
                    }
                    if starts_with(source, probe, tag) {
                        break probe;
                    }
                    probe += 1;
                };
                out.text(start..close, false, true, 0);
                at = close + tag.len();
            }
            _ => at += char_width(source, at),
        }
    }
    Ok(out)
}

fn haskell(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        let byte = source[at];
        if starts_with(source, at, b"--") {
            let mut cursor = at;
            while source.get(cursor) == Some(&b'-') {
                cursor += 1;
            }
            let tail = source.get(cursor).copied().unwrap_or(b'\n');
            if !SYMBOLS.contains(&tail) {
                let end = line_end(source, at);
                out.comment(at..end, at + 2..end);
                at = end;
                continue;
            }
            at = cursor;
            continue;
        }
        if starts_with(source, at, b"{-") {
            let start = at;
            let mut cursor = at + 2;
            let mut depth = 1usize;
            while cursor < source.len() {
                if starts_with(source, cursor, b"{-") {
                    depth += 1;
                    cursor += 2;
                    continue;
                }
                if starts_with(source, cursor, b"-}") {
                    cursor += 2;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                cursor += 1;
            }
            if depth > 0 {
                return Err(Bail("unterminated block comment"));
            }
            out.comment(start..cursor, start + 2..cursor - 2);
            at = cursor;
            continue;
        }
        match byte {
            b'"' => {
                let start = at + 1;
                let mut cursor = start;
                loop {
                    if cursor >= source.len() {
                        return Err(Bail("unterminated string"));
                    }
                    match source[cursor] {
                        b'\\' => {
                            cursor += 1 + char_width(source, (cursor + 1).min(source.len() - 1))
                        }
                        b'"' => {
                            out.text(start..cursor, true, true, b'"');
                            at = cursor + 1;
                            break;
                        }
                        b'\n' => return Err(Bail("unterminated string")),
                        _ => cursor += char_width(source, cursor),
                    }
                }
            }
            b'\'' if at == 0 || !identifier_byte(source[at - 1]) => {
                let mut cursor = at + 1;
                loop {
                    if cursor >= source.len() || source[cursor] == b'\n' {
                        at += 1;
                        break;
                    }
                    match source[cursor] {
                        b'\\' => {
                            cursor += 1 + char_width(source, (cursor + 1).min(source.len() - 1))
                        }
                        b'\'' => {
                            at = cursor + 1;
                            break;
                        }
                        _ => cursor += char_width(source, cursor),
                    }
                }
            }
            _ => at += char_width(source, at),
        }
    }
    Ok(out)
}
