use std::io::{self, Write};

use memchr::memchr;
use rustc_hash::FxHashMap;
use vte::{Params, Parser, Perform};

use crate::colors::{self, Swatch};
use crate::foreground::Foreground;

pub const SWATCH: &str = "██";
const ESC: u8 = 0x1b;
const REMEMBERED: usize = 1024;

#[derive(Default)]
pub struct Annotator {
    parser: Parser,
    scan: Scan,
    swatches: Vec<Swatch>,
    // What each CSI sequence does to the foreground: highlighters repeat a few dozen.
    sequences: FxHashMap<Box<[u8]>, Option<Foreground>>,
}

#[derive(Default)]
struct Scan {
    text: String,
    spans: Vec<Span>,
    offset: usize,
    foreground: Foreground,
    set: Option<Foreground>,
    // vte may be partway through a sequence or character, so bytes go through it one by one.
    mid_sequence: bool,
    ended: bool,
}

// Visible text from `text` on starts at `origin` in the raw line.
#[derive(Clone, Copy)]
struct Span {
    text: usize,
    origin: Origin,
}

#[derive(Clone, Copy)]
struct Origin {
    offset: usize,
    foreground: Foreground,
}

impl Annotator {
    pub fn annotate(&mut self, line: &[u8], output: &mut impl Write) -> io::Result<()> {
        let utf8 = std::str::from_utf8(line).ok();
        match utf8 {
            Some(text) if !self.scan.mid_sequence && memchr(ESC, line).is_none() => {
                colors::find(text, &mut self.swatches);
                let foreground = self.scan.foreground;
                let origin = |offset| Origin { offset, foreground };
                insert(line, &self.swatches, origin, output)
            }
            _ => {
                self.scan(line, utf8);
                colors::find(&self.scan.text, &mut self.swatches);
                let origin = |start| self.scan.origin(start);
                insert(line, &self.swatches, origin, output)
            }
        }
    }

    fn scan(&mut self, line: &[u8], utf8: Option<&str>) {
        self.scan.text.clear();
        self.scan.spans.clear();
        let mut at = 0;
        while at < line.len() {
            at = if self.scan.mid_sequence {
                self.step(line, at)
            } else if line[at] == ESC {
                self.sequence(line, at)
            } else {
                self.text(line, utf8, at)
            };
        }
    }

    // Valid UTF-8 up to the next escape is exactly what vte would print.
    fn text(&mut self, line: &[u8], utf8: Option<&str>, start: usize) -> usize {
        let end = memchr(ESC, &line[start..]).map_or(line.len(), |length| start + length);
        let text = utf8
            .and_then(|utf8| utf8.get(start..end))
            .unwrap_or_else(|| valid_prefix(&line[start..end]));
        self.scan.mid_sequence = text.len() < end - start;
        self.scan.push(text, start);
        start + text.len()
    }

    // A plain CSI sequence holds no text, so it can go through vte in one piece.
    fn sequence(&mut self, line: &[u8], start: usize) -> usize {
        let Some(length) = csi_length(&line[start..]) else {
            self.scan.mid_sequence = true;
            return start;
        };
        let sequence = &line[start..start + length];
        match self.sequences.get(sequence) {
            Some(&set) => self.scan.foreground = set.unwrap_or(self.scan.foreground),
            None => {
                self.scan.set = None;
                self.parser.advance(&mut self.scan, sequence);
                if self.sequences.len() < REMEMBERED {
                    self.sequences.insert(sequence.into(), self.scan.set);
                }
            }
        }
        start + length
    }

    fn step(&mut self, line: &[u8], at: usize) -> usize {
        let byte = line[at];
        self.scan.offset = at;
        self.scan.ended = false;
        self.parser
            .advance(&mut self.scan, std::slice::from_ref(&byte));
        // Only an ASCII byte that ends a sequence or character leaves vte with nothing pending.
        if self.scan.ended && byte.is_ascii() && byte != ESC {
            self.scan.mid_sequence = false;
        }
        at + 1
    }
}

// vte replaces invalid or cut-off characters itself.
fn valid_prefix(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
    }
}

fn csi_length(bytes: &[u8]) -> Option<usize> {
    let [ESC, b'[', rest @ ..] = bytes else {
        return None;
    };
    let body = rest
        .iter()
        .position(|byte| !(0x20..=0x3f).contains(byte))?;
    (0x40..=0x7e).contains(&rest[body]).then_some(body + 3)
}

fn insert(
    line: &[u8],
    swatches: &[Swatch],
    origin: impl Fn(usize) -> Origin,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut written = 0;
    for swatch in swatches {
        let [r, g, b] = swatch.rgb;
        let Origin { offset, foreground } = origin(swatch.start);
        output.write_all(&line[written..offset])?;
        write!(output, "\x1b[38;2;{r};{g};{b}m{SWATCH}{foreground} ")?;
        written = offset;
    }
    output.write_all(&line[written..])
}

impl Scan {
    fn push(&mut self, visible: &str, offset: usize) {
        let origin = Origin {
            offset,
            foreground: self.foreground,
        };
        self.spans.push(Span {
            text: self.text.len(),
            origin,
        });
        self.text.push_str(visible);
    }

    fn origin(&self, start: usize) -> Origin {
        let span = self.spans[self.spans.partition_point(|span| span.text <= start) - 1];
        Origin {
            offset: span.origin.offset + start - span.text,
            ..span.origin
        }
    }
}

impl Perform for Scan {
    fn print(&mut self, visible: char) {
        self.ended = true;
        self.push(visible.encode_utf8(&mut [0; 4]), self.offset);
    }

    fn execute(&mut self, control: u8) {
        self.push(char::from(control).encode_utf8(&mut [0; 4]), self.offset);
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _: bool, action: char) {
        self.ended = true;
        let sgr = action == 'm' && intermediates.is_empty();
        self.set = sgr.then(|| Foreground::set_by(params)).flatten();
        self.foreground = self.set.unwrap_or(self.foreground);
    }

    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
        self.ended = true;
    }

    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {
        self.ended = true;
    }
}

#[cfg(test)]
#[path = "../tests/unit/annotate_tests.rs"]
mod tests;
