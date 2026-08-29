use crate::edit::{self, Cut, Edited, Kind};
use crate::glyphs;
use crate::keep::{self, Kept};
use crate::lang::Dialect;
use crate::scan;
use crate::{docstring, scan::Comment};

#[derive(Default, Clone, Copy)]
pub struct Saved {
    pub shebangs: usize,
    pub directives: usize,
    pub licenses: usize,
}

impl Saved {
    pub fn any(&self) -> bool {
        self.shebangs + self.directives + self.licenses > 0
    }

    pub fn add(&mut self, other: Saved) {
        self.shebangs += other.shebangs;
        self.directives += other.directives;
        self.licenses += other.licenses;
    }
}

pub enum Outcome {
    Changed(Box<Edited>, Saved),
    Untouched(Saved),
    Skipped(&'static str),
}

fn leading(source: &[u8], comments: &[Comment]) -> usize {
    let mut cursor = 0usize;
    let mut count = 0usize;
    for comment in comments {
        if source[cursor..comment.span.start]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            break;
        }
        cursor = comment.span.end;
        count += 1;
    }
    count
}

pub fn purge(source: &[u8], dialect: Dialect) -> Outcome {
    let out = match scan::scan(source, dialect) {
        Ok(out) => out,
        Err(bail) => return Outcome::Skipped(bail.0),
    };
    let mut saved = Saved::default();
    let mut cuts: Vec<Cut> = Vec::new();
    let heading = leading(source, &out.comments);
    for (index, comment) in out.comments.iter().enumerate() {
        match keep::kept(source, comment, dialect, index < heading) {
            Some(Kept::Shebang) => {
                saved.shebangs += 1;
                continue;
            }
            Some(Kept::Directive) => {
                saved.directives += 1;
                continue;
            }
            Some(Kept::License) => {
                saved.licenses += 1;
                continue;
            }
            None => {}
        }
        let (span, with) = match edit::whole_lines(source, &comment.span) {
            Some(widened) => (widened, None),
            None => {
                let mut start = comment.span.start;
                while start > 0 && matches!(source[start - 1], b' ' | b'\t') {
                    start -= 1;
                }
                let mut end = comment.span.end;
                while end < source.len() && matches!(source[end], b' ' | b'\t') {
                    end += 1;
                }
                let padded = start < comment.span.start && end > comment.span.end;
                (start..end, padded.then(|| " ".to_string()))
            }
        };
        cuts.push(Cut {
            span,
            with,
            kind: Kind::Comment,
            lines: false,
        });
    }
    cuts.extend(docstring::cuts(source, &out));
    for text in &out.texts {
        for swap in glyphs::sweep(source, text) {
            cuts.push(Cut {
                span: swap.span,
                with: (!swap.with.is_empty()).then_some(swap.with),
                kind: Kind::Glyph,
                lines: false,
            });
        }
    }
    let edited = edit::apply(source, cuts);
    if !edited.touched() {
        return Outcome::Untouched(saved);
    }
    if scan::scan(&edited.content, dialect).is_err() {
        return Outcome::Skipped("the purged file would not read back");
    }
    Outcome::Changed(Box::new(edited), saved)
}
