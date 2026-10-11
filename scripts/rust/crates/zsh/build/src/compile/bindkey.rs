//! Key bindings as ledger names: `KEYMAP KEY`, the key spelled as `^X`,
//! `^?` and `\M-`, either part `*` when only known at runtime. `main`,
//! `emacs` and `viins` count as one keymap, as `main` links to either.

use super::ledger::{ANY, Key, Touch, static_name};

pub(crate) const KIND: &str = "bindkey";

/// What `bindkey` with these argument values binds, removes or reads.
pub(crate) fn touches(values: &[String]) -> Vec<(Touch, Key)> {
    let mut flags = String::new();
    let mut keymap = None;
    let mut operands: Vec<&str> = Vec::new();
    let mut rest = values.iter();
    while let Some(value) = rest.next() {
        match value.strip_prefix('-') {
            Some("-") if operands.is_empty() => operands.extend(rest.by_ref().map(String::as_str)),
            Some(options) if operands.is_empty() && !options.is_empty() => {
                if let Some((before, attached)) = options.split_once('M') {
                    flags.push_str(before);
                    keymap = match attached {
                        "" => rest.next().cloned(),
                        attached => Some(attached.to_string()),
                    };
                } else {
                    flags.push_str(options);
                }
            }
            _ => operands.push(value),
        }
    }
    let selected = keymap.as_deref().unwrap_or(match () {
        _ if flags.contains('e') => "emacs",
        _ if flags.contains('v') => "viins",
        _ if flags.contains('a') => "vicmd",
        _ => "main",
    });
    let map = group(selected);
    let define = |map: &str, key: &str| (Touch::Define, (KIND, format!("{map} {key}")));
    let read = |map: &str, key: &str| (Touch::Call, (KIND, format!("{map} {key}")));
    let dynamic = operands.iter().any(|operand| static_name(operand) == ANY);
    let mut out: Vec<(Touch, Key)> = if flags.contains(['e', 'v']) {
        vec![define("main", ANY)]
    } else {
        Vec::new()
    };
    let keys = |operands: &[&str], step: usize| -> Vec<(Touch, Key)> {
        operands
            .iter()
            .step_by(step)
            .map(|key| match flags.contains('R') {
                true => define(&map, ANY),
                false => define(&map, &key_name(key)),
            })
            .collect()
    };
    out.extend(match () {
        _ if flags.contains('d') => vec![define(ANY, ANY)],
        _ if flags.contains('l') => Vec::new(),
        _ if flags.contains('N') => operands
            .iter()
            .take(1)
            .map(|new| define(&group(new), ANY))
            .collect(),
        _ if flags.contains('A') => operands
            .iter()
            .skip(1)
            .take(1)
            .map(|new| define(&group(new), ANY))
            .collect(),
        _ if flags.contains('D') => operands
            .iter()
            .map(|map| define(&group(map), ANY))
            .collect(),
        _ if flags.contains('m') => vec![define(&map, ANY)],
        _ if dynamic => vec![define(&map, ANY), read(&map, ANY)],
        _ if flags.contains('r') => keys(&operands, 1),
        _ if flags.contains('s') => keys(&operands, 2),
        _ if flags.contains(['L', 'p']) => vec![read(&map, ANY)],
        _ => match operands.as_slice() {
            [] if flags.contains(['e', 'v']) => Vec::new(),
            [] => vec![read(&map, ANY)],
            [key] => vec![read(&map, &key_name(key))],
            _ => keys(&operands, 2),
        },
    });
    out
}

/// Whether binding name `own` stands for `name`, through a `*` keymap or key.
pub(crate) fn covers(own: &str, name: &str) -> bool {
    match (own.split_once(' '), name.split_once(' ')) {
        (Some((own_map, own_key)), Some((map, key))) => {
            (own_map == ANY || own_map == map) && (own_key == ANY || own_key == key)
        }
        _ => false,
    }
}

fn group(keymap: &str) -> String {
    match keymap {
        "main" | "emacs" | "viins" => "main".to_string(),
        keymap => static_name(keymap),
    }
}

/// The key sequence bindkey reads from `spec`, in one spelling.
pub(crate) fn key_name(spec: &str) -> String {
    if static_name(spec) == ANY {
        return ANY.to_string();
    }
    let mut out = String::new();
    for byte in key_bytes(spec) {
        if byte & 0x80 != 0 {
            out.push_str("\\M-");
        }
        match byte & 0x7f {
            0x7f => out.push_str("^?"),
            control @ 0..0x20 => {
                out.push('^');
                out.push(char::from(control + 0x40));
            }
            b'\\' => out.push_str("\\\\"),
            plain => out.push(char::from(plain)),
        }
    }
    out
}

/// Bytes of a key spec, with bindkey's `\` escapes, `\M-`, `\C-` and `^X` read.
fn key_bytes(spec: &str) -> Vec<u8> {
    let bytes = spec.as_bytes();
    let (mut out, mut meta, mut control, mut index) = (Vec::new(), false, false, 0);
    while index < bytes.len() {
        let mut byte = bytes[index];
        index += 1;
        if byte == b'^' && !control && index < bytes.len() {
            control = true;
            continue;
        }
        if byte == b'\\' && index < bytes.len() {
            let (escape, next) = (bytes[index], bytes.get(index + 1));
            index += 1;
            byte = match escape {
                b'M' | b'C' if next == Some(&b'-') && index + 1 < bytes.len() => {
                    index += 1;
                    meta |= escape == b'M';
                    control |= escape == b'C';
                    continue;
                }
                b'a' => 0x07,
                b'b' => 0x08,
                b'e' | b'E' => 0x1b,
                b'f' => 0x0c,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'v' => 0x0b,
                b'x' => number(bytes, &mut index, 16, 2),
                b'0'..=b'7' => {
                    index -= 1;
                    number(bytes, &mut index, 8, 3)
                }
                other => other,
            };
        }
        if control {
            byte = if byte == b'?' { 0x7f } else { byte & 0x9f };
        }
        if meta {
            byte |= 0x80;
        }
        (meta, control) = (false, false);
        out.push(byte);
    }
    out
}

fn number(bytes: &[u8], index: &mut usize, radix: u32, digits: usize) -> u8 {
    let mut value = 0u32;
    for _ in 0..digits {
        let Some(digit) = bytes
            .get(*index)
            .and_then(|byte| char::from(*byte).to_digit(radix))
        else {
            break;
        };
        value = value * radix + digit;
        *index += 1;
    }
    value as u8
}

#[cfg(test)]
#[path = "../../tests/unit/compile/bindkey_tests.rs"]
mod tests;
