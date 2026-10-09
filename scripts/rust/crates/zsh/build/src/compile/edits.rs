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
    /// File-level returns in statements dropped by `# zsh-build: omit`.
    pub returns_omitted: usize,
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
        self.render_with(script, 0..script.text.len(), |text| text)
    }

    /// The rendering of the edits within `bounds`, a range of whole statements.
    pub fn render_within(&self, script: &Script, bounds: Range<usize>) -> String {
        self.render_with(script, bounds, |text| text)
    }

    /// The rendering with each replaced statement stubbed out: what remains
    /// to check once every replacement was checked on its own.
    pub fn skeleton(&self, script: &Script) -> String {
        self.render_with(script, 0..script.text.len(), |text| {
            if text.is_empty() { "" } else { ":\n" }
        })
    }

    fn render_with<'a>(
        &'a self,
        script: &Script,
        bounds: Range<usize>,
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
            .filter(|(range, _)| bounds.start <= range.start && range.end <= bounds.end)
            .collect();
        // Insertions sort ahead of a replacement starting at the same offset.
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut out = String::with_capacity(bounds.len());
        let mut position = bounds.start;
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
        out.push_str(&script.text[position..bounds.end]);
        out
    }
}
