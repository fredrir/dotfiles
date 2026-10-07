//! Writing a value out the way jq writes one.
//!
//! Checked against `jq --indent 2 .` on every tracked `.json` file in this
//! repository, and against `jq -c` for `indent = 0`: two spaces a level, `": "`
//! between a key and its value, one member a line, `{}` and `[]` inline, and
//! nothing escaped that jq would leave alone.

use crate::value::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Indent {
    /// `indent = 0`, which is jq's `--indent 0`: one line, no spaces.
    Compact,
    Spaces(usize),
    /// `indent = -1`, which is jq's.
    Tabs,
}

#[derive(Clone, Copy)]
pub struct Layout {
    pub indent: Indent,
    pub final_newline: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            indent: Indent::Spaces(2),
            final_newline: true,
        }
    }
}

pub fn write(value: &Value, layout: Layout) -> String {
    let mut out = String::new();
    value_into(&mut out, value, layout, 0);
    if layout.final_newline {
        out.push('\n');
    }
    out
}

fn value_into(out: &mut String, value: &Value, layout: Layout, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(literal) => out.push_str(literal),
        Value::String(text) => string_into(out, text),
        Value::Array(items) => array_into(out, items, layout, depth),
        Value::Object(entries) => object_into(out, entries, layout, depth),
    }
}

fn array_into(out: &mut String, items: &[Value], layout: Layout, depth: usize) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push('[');
    for (at, item) in items.iter().enumerate() {
        if at > 0 {
            out.push(',');
        }
        open_line(out, layout, depth + 1);
        value_into(out, item, layout, depth + 1);
    }
    close_line(out, layout, depth);
    out.push(']');
}

fn object_into(
    out: &mut String,
    entries: &indexmap::IndexMap<String, Value>,
    layout: Layout,
    depth: usize,
) {
    if entries.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for (at, (key, value)) in entries.iter().enumerate() {
        if at > 0 {
            out.push(',');
        }
        open_line(out, layout, depth + 1);
        string_into(out, key);
        out.push(':');
        if layout.indent != Indent::Compact {
            out.push(' ');
        }
        value_into(out, value, layout, depth + 1);
    }
    close_line(out, layout, depth);
    out.push('}');
}

fn open_line(out: &mut String, layout: Layout, depth: usize) {
    if layout.indent == Indent::Compact {
        return;
    }
    out.push('\n');
    indent(out, layout, depth);
}

fn close_line(out: &mut String, layout: Layout, depth: usize) {
    open_line(out, layout, depth);
}

fn indent(out: &mut String, layout: Layout, depth: usize) {
    match layout.indent {
        Indent::Compact => {}
        Indent::Spaces(width) => out.push_str(&" ".repeat(width * depth)),
        Indent::Tabs => out.push_str(&"\t".repeat(depth)),
    }
}

/// jq escapes the quote, the backslash, the control characters a JSON string
/// cannot hold raw, and DEL — which JSON allows and jq writes as `\u007f`.
fn string_into(out: &mut String, text: &str) {
    out.push('"');
    let bytes = text.as_bytes();
    let mut plain = 0;
    for (at, byte) in bytes.iter().enumerate() {
        if !ESCAPED[*byte as usize] {
            continue;
        }
        out.push_str(&text[plain..at]);
        write_escape(out, *byte);
        plain = at + 1;
    }
    out.push_str(&text[plain..]);
    out.push('"');
}

fn write_escape(out: &mut String, byte: u8) {
    match byte {
        b'"' => out.push_str("\\\""),
        b'\\' => out.push_str("\\\\"),
        0x08 => out.push_str("\\b"),
        0x09 => out.push_str("\\t"),
        0x0a => out.push_str("\\n"),
        0x0c => out.push_str("\\f"),
        0x0d => out.push_str("\\r"),
        other => {
            out.push_str("\\u00");
            out.push(char::from_digit(u32::from(other >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(other & 0xf), 16).unwrap_or('0'));
        }
    }
}

/// The scan behind `string_into` is a byte loop, so the answer it asks has to
/// be a lookup rather than a comparison against four things.
static ESCAPED: [bool; 256] = escaped();

const fn escaped() -> [bool; 256] {
    let mut table = [false; 256];
    let mut byte = 0;
    while byte < 0x20 {
        table[byte] = true;
        byte += 1;
    }
    table[0x7f] = true;
    table[b'"' as usize] = true;
    table[b'\\' as usize] = true;
    table
}
