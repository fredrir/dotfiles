use std::ops::{Range, RangeInclusive};

use crate::alias;
use crate::script::Script;

#[derive(Default)]
pub struct Edits {
    lines: Vec<(RangeInclusive<usize>, String)>,
    bytes: Vec<(Range<usize>, String)>,
    /// Alias expansions, by absolute byte range; not counted by `len`.
    pub aliases: Vec<alias::Edit>,
    /// Statements whose aliases were handled, expanded or left.
    pub alias_done: Vec<Range<usize>>,
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

    /// A position to ask what was replaced since.
    pub fn mark(&self) -> (usize, usize, usize) {
        (self.lines.len(), self.bytes.len(), self.alias_done.len())
    }

    /// Byte ranges replaced, or whose aliases were handled, since `mark`.
    pub fn handled_since(&self, script: &Script, mark: (usize, usize, usize)) -> Vec<Range<usize>> {
        let (lines, bytes, done) = mark;
        self.lines[lines..]
            .iter()
            .map(|(lines, _)| script.byte_range(lines))
            .chain(self.bytes[bytes..].iter().map(|(range, _)| range.clone()))
            .chain(self.alias_done[done..].iter().cloned())
            .collect()
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
        let mut edits: Vec<(Range<usize>, &str, bool)> = self
            .lines
            .iter()
            .map(|(lines, text)| (script.byte_range(lines), line_text(text.as_str()), false))
            .chain(
                self.bytes
                    .iter()
                    .map(|(range, text)| (range.clone(), text.as_str(), false)),
            )
            .chain(
                self.aliases
                    .iter()
                    .map(|edit| (edit.range.clone(), edit.protected.as_str(), true)),
            )
            .filter(|(range, ..)| bounds.start <= range.start && range.end <= bounds.end)
            .collect();
        // Insertions sort ahead of a replacement starting at the same offset, which an alias edit yields to.
        edits.sort_by_key(|(range, _, alias)| {
            (range.start, range.end != range.start, *alias, range.end)
        });
        let mut out = String::with_capacity(bounds.len());
        let mut position = bounds.start;
        for (range, text, _) in edits {
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
