use std::io::{self, BufRead};

use anstream::adapter::strip_bytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Unified,
    Split,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Section {
    pub text: Vec<u8>,
    // `None` when nothing changed, so the section can join either layout.
    pub layout: Option<Layout>,
}

pub struct Sections<R> {
    input: R,
    next_header: Option<Vec<u8>>,
}

impl<R: BufRead> Sections<R> {
    pub fn new(input: R) -> Sections<R> {
        Sections {
            input,
            next_header: None,
        }
    }
}

impl<R: BufRead> Iterator for Sections<R> {
    type Item = io::Result<Section>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut changes = Changes::default();
        let mut text = Vec::new();
        if let Some(header) = self.next_header.take() {
            changes.read(&strip_bytes(&header).into_vec());
            text = header;
        }
        loop {
            let mut line = Vec::new();
            match self.input.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => return Some(Err(error)),
            }
            let visible = strip_bytes(&line).into_vec();
            if !text.is_empty() && starts_file(&visible) {
                self.next_header = Some(line);
                break;
            }
            changes.read(&visible);
            match hunk_range(&visible) {
                Some(range) => {
                    text.extend_from_slice(range);
                    text.push(b'\n');
                }
                None => text.extend_from_slice(&line),
            }
        }
        (!text.is_empty()).then(|| {
            Ok(Section {
                layout: changes.layout(),
                text,
            })
        })
    }
}

// git follows the range with the enclosing code, as in `@@ -1,2 +1,3 @@ fn main() {`.
fn hunk_range(line: &[u8]) -> Option<&[u8]> {
    let ranges = line.strip_prefix(b"@@ ")?;
    let end = ranges.windows(3).position(|window| window == b" @@")?;
    Some(&line[..b"@@ ".len() + end + b" @@".len()])
}

fn starts_file(line: &[u8]) -> bool {
    line.starts_with(b"diff --git ") || is_combined(line)
}

fn is_combined(line: &[u8]) -> bool {
    line.starts_with(b"diff --cc ") || line.starts_with(b"diff --combined ")
}

#[derive(Default)]
struct Changes {
    combined: bool,
    in_hunk: bool,
    removed: bool,
    added: bool,
}

impl Changes {
    fn read(&mut self, line: &[u8]) {
        if is_combined(line) {
            self.combined = true;
        } else if line.starts_with(b"@@") {
            self.in_hunk = true;
        } else if self.in_hunk {
            match line.first() {
                Some(b'-') => self.removed = true,
                Some(b'+') => self.added = true,
                _ => {}
            }
        }
    }

    fn layout(&self) -> Option<Layout> {
        match (self.combined, self.removed, self.added) {
            (false, false, false) => None,
            (false, true, true) => Some(Layout::Split),
            _ => Some(Layout::Unified),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/sections_tests.rs"]
mod tests;
