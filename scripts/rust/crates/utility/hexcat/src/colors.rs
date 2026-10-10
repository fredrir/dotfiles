use std::sync::LazyLock;

use memchr::memchr;
use regex::Regex;

// Kept apart: one alternation of all three searches about ten times slower.
static CODES: LazyLock<[Code; 3]> = LazyLock::new(|| {
    [
        Code::new(
            b"#",
            r"(?-u:\B)\#(?:[[:xdigit:]]{8}|[[:xdigit:]]{6}|[[:xdigit:]]{3,4})(?-u:\b)",
        ),
        Code::new(b"xX", r"(?-u:\b)0[xX][[:xdigit:]]{6}(?-u:\b)"),
        Code::new(b"(", r"(?-u:\b)(?i-u:rgba?|hsla?)\([^()\n]*\)"),
    ]
});

struct Code {
    marks: &'static [u8],
    pattern: Regex,
}

impl Code {
    fn new(marks: &'static [u8], pattern: &str) -> Self {
        Self {
            marks,
            pattern: Regex::new(pattern).expect("color pattern compiles"),
        }
    }

    // A byte scan is far cheaper than a regex search that finds nothing.
    fn may_match(&self, text: &str) -> bool {
        self.marks
            .iter()
            .any(|&mark| memchr(mark, text.as_bytes()).is_some())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swatch {
    pub start: usize,
    pub rgb: [u8; 3],
}

pub fn find(text: &str, swatches: &mut Vec<Swatch>) {
    swatches.clear();
    for code in CODES.iter().filter(|code| code.may_match(text)) {
        swatches.extend(code.pattern.find_iter(text).filter_map(|found| {
            Some(Swatch {
                start: found.start(),
                rgb: rgb(found.as_str())?,
            })
        }));
    }
    swatches.sort_unstable_by_key(|swatch| swatch.start);
}

fn rgb(code: &str) -> Option<[u8; 3]> {
    if let Some(digits) = code.strip_prefix('#') {
        return hex(digits).filter(|_| !reads_as_issue_number(digits));
    }
    if let Some(digits) = code.strip_prefix("0x").or_else(|| code.strip_prefix("0X")) {
        return hex(digits);
    }
    let [r, g, b, _] = csscolorparser::parse(&code.to_ascii_lowercase())
        .ok()?
        .to_rgba8();
    Some([r, g, b])
}

// `rgb`, `rgba`, `rrggbb` or `rrggbbaa`; alpha has no place on a swatch.
fn hex(digits: &str) -> Option<[u8; 3]> {
    let value = u32::from_str_radix(digits, 16).ok()?;
    let nibble = |index: u32| ((value >> (4 * index)) & 0xf) as u8 * 0x11;
    let byte = |index: u32| (value >> (8 * index)) as u8;
    match digits.len() {
        3 => Some([nibble(2), nibble(1), nibble(0)]),
        4 => Some([nibble(3), nibble(2), nibble(1)]),
        6 => Some([byte(2), byte(1), byte(0)]),
        8 => Some([byte(3), byte(2), byte(1)]),
        _ => None,
    }
}

// `#123` is far more often an issue or PR than a color; `#333` stays a color.
fn reads_as_issue_number(digits: &str) -> bool {
    let bytes = digits.as_bytes();
    bytes.len() <= 4
        && bytes.iter().all(u8::is_ascii_digit)
        && bytes.iter().any(|&digit| digit != bytes[0])
}

#[cfg(test)]
#[path = "../tests/unit/colors_tests.rs"]
mod tests;
