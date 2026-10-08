//! Structured counts of repairs performed while parsing.

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
}
