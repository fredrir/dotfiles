use std::ops::Range;

use crate::scan::Text;

const DROPPED: &[char] = &[
    '\u{2014}', '\u{2013}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}',
];

pub struct Swap {
    pub span: Range<usize>,
    pub with: String,
}

fn folded(letter: char) -> Option<char> {
    match letter {
        '\u{201c}' | '\u{201d}' => Some('"'),
        '\u{2018}' | '\u{2019}' => Some('\''),
        '\u{00a0}' | '\u{2007}' | '\u{202f}' => Some(' '),
        _ => None,
    }
}

fn dropped(letter: char) -> bool {
    DROPPED.contains(&letter)
}

pub fn sweep(source: &[u8], text: &Text) -> Vec<Swap> {
    let Ok(body) = std::str::from_utf8(&source[text.body.clone()]) else {
        return Vec::new();
    };
    let base = text.body.start;
    let mut swaps = Vec::new();
    let letters: Vec<(usize, char)> = body.char_indices().collect();
    let mut index = 0usize;
    while index < letters.len() {
        let (offset, letter) = letters[index];
        if let Some(target) = folded(letter) {
            index += 1;
            if target == '"' || target == '\'' {
                if !text.folds {
                    continue;
                }
                let replacement = if target as u8 == text.quote {
                    if !text.escapes {
                        continue;
                    }
                    format!("\\{target}")
                } else {
                    target.to_string()
                };
                swaps.push(Swap {
                    span: base + offset..base + offset + letter.len_utf8(),
                    with: replacement,
                });
            } else {
                swaps.push(Swap {
                    span: base + offset..base + offset + letter.len_utf8(),
                    with: target.to_string(),
                });
            }
            continue;
        }
        if !dropped(letter) {
            index += 1;
            continue;
        }
        let mut last = index;
        while last + 1 < letters.len() && dropped(letters[last + 1].1) {
            last += 1;
        }
        let mut left = index;
        while left > 0 && matches!(letters[left - 1].1, ' ' | '\t') {
            left -= 1;
        }
        let mut right = last;
        while right + 1 < letters.len() && matches!(letters[right + 1].1, ' ' | '\t') {
            right += 1;
        }
        let head = left == 0 || letters[left - 1].1 == '\n';
        let tail = right + 1 == letters.len() || letters[right + 1].1 == '\n';
        let padded = left < index || right > last;
        let (opens, with) = match (head, tail) {
            (true, false) => (index, ""),
            (false, false) if padded => (left, " "),
            _ => (left, ""),
        };
        let from = letters[opens].0;
        let upto = letters[right].0 + letters[right].1.len_utf8();
        swaps.push(Swap {
            span: base + from..base + upto,
            with: with.to_string(),
        });
        index = last + 1;
    }
    swaps
}

pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let letters: Vec<char> = text.chars().collect();
    let mut index = 0usize;
    while index < letters.len() {
        let letter = letters[index];
        if let Some(target) = folded(letter) {
            out.push(target);
            index += 1;
            continue;
        }
        if !dropped(letter) {
            out.push(letter);
            index += 1;
            continue;
        }
        let mut last = index;
        while last + 1 < letters.len() && dropped(letters[last + 1]) {
            last += 1;
        }
        let padded =
            out.ends_with([' ', '\t']) || matches!(letters.get(last + 1), Some(' ') | Some('\t'));
        while out.ends_with([' ', '\t']) {
            out.pop();
        }
        let mut next = last + 1;
        while matches!(letters.get(next), Some(' ') | Some('\t')) {
            next += 1;
        }
        if padded && !out.is_empty() && next < letters.len() {
            out.push(' ');
        }
        index = next;
    }
    out
}
