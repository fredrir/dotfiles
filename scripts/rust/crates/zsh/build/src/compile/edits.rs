use std::ops::{Range, RangeInclusive};

use crate::script::Script;

#[derive(Default)]
pub struct Edits {
    lines: Vec<(RangeInclusive<usize>, String)>,
    bytes: Vec<(Range<usize>, String)>,
    /// Byte ranges of top-level function definitions, where `$0` is the function.
    pub functions: Vec<Range<usize>>,
    /// Start offsets of `$0` references already rewritten.
    pub zero_done: Vec<usize>,
    pub returns_rewritten: usize,
}

impl Edits {
    pub fn replace_lines(&mut self, lines: RangeInclusive<usize>, text: String) {
        self.lines.push((lines, text));
    }

    pub fn replace_bytes(&mut self, range: Range<usize>, text: String) {
        self.bytes.push((range, text));
    }

    pub fn len(&self) -> usize {
        self.lines.len() + self.bytes.len()
    }

    pub fn in_function(&self, offset: usize) -> bool {
        self.functions.iter().any(|range| range.contains(&offset))
    }

    pub fn render(&self, script: &Script) -> String {
        self.render_with(script, |text| text)
    }

    /// The rendering with each replaced statement stubbed out: what remains
    /// to check once every replacement was checked on its own.
    pub fn skeleton(&self, script: &Script) -> String {
        self.render_with(script, |text| if text.is_empty() { "" } else { ":\n" })
    }

    fn render_with<'a>(
        &'a self,
        script: &Script,
        line_text: impl Fn(&'a str) -> &'a str,
    ) -> String {
        let mut edits: Vec<(Range<usize>, &str)> = self
            .lines
            .iter()
            .map(|(lines, text)| (script.byte_range(lines), line_text(text.as_str())))
            .chain(
                self.bytes
                    .iter()
                    .map(|(range, text)| (range.clone(), text.as_str())),
            )
            .collect();
        edits.sort_by_key(|(range, _)| range.start);
        let mut out = String::with_capacity(script.text.len());
        let mut position = 0;
        for (range, text) in edits {
            if range.start < position {
                continue;
            }
            out.push_str(&script.text[position..range.start]);
            out.push_str(text);
            let replaced = &script.text[range.clone()];
            if replaced.ends_with('\n') && !text.is_empty() && !text.ends_with('\n') {
                out.push('\n');
            }
            position = range.end;
        }
        out.push_str(&script.text[position..]);
        out
    }
}
