//! What `--editor` fixed, and how it is said.

use std::fmt::Write;

use workstation::text::plural;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Repair {
    Comma,
    MissingComma,
    Comment,
    Quote,
    Key,
    Literal,
    Surrogate,
    Control,
    Space,
    Utf8,
}

impl Repair {
    pub const ALL: [Repair; 10] = [
        Repair::Comma,
        Repair::MissingComma,
        Repair::Comment,
        Repair::Quote,
        Repair::Key,
        Repair::Literal,
        Repair::Surrogate,
        Repair::Control,
        Repair::Space,
        Repair::Utf8,
    ];

    /// Singular and plural, because the note counts rather than explains.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Repair::Comma => ("stray comma", "stray commas"),
            Repair::MissingComma => ("missing comma", "missing commas"),
            Repair::Comment => ("comment", "comments"),
            Repair::Quote => ("single quote", "single quotes"),
            Repair::Key => ("unquoted key", "unquoted keys"),
            Repair::Literal => ("literal", "literals"),
            Repair::Surrogate => ("lone surrogate", "lone surrogates"),
            Repair::Control => ("control character", "control characters"),
            Repair::Space => ("unusual space", "unusual spaces"),
            Repair::Utf8 => ("invalid byte", "invalid bytes"),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Repairs {
    counts: [usize; Repair::ALL.len()],
}

impl Repairs {
    pub fn add(&mut self, repair: Repair) {
        self.counts[repair as usize] += 1;
    }

    pub fn of(&self, repair: Repair) -> usize {
        self.counts[repair as usize]
    }

    pub fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// `2 stray commas, 1 comment`, in a fixed order so two runs read the same.
    pub fn describe(&self) -> String {
        let mut said = String::new();
        for repair in Repair::ALL {
            let count = self.of(repair);
            if count == 0 {
                continue;
            }
            if !said.is_empty() {
                said.push_str(", ");
            }
            let (one, many) = repair.names();
            let _ = write!(said, "{count} {}", plural(count, one, many));
        }
        said
    }
}
