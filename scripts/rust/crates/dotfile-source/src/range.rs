use std::fmt::{self, Display, Formatter};

/// A checked zero-based, half-open `u64` byte range over the original source
/// bytes.
///
/// Every range satisfies `start <= end <= source_length`. This is the
/// canonical span representation for tokens, CST nodes, semantic anchors,
/// diagnostics, lock spans, hashes, and stable IDs (ADR 0002).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    /// Creates the range `start..end`, checked against the source length.
    pub fn new(start: u64, end: u64, source_len: u64) -> Option<Self> {
        if start <= end && end <= source_len {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// Creates the zero-width range `offset..offset`.
    pub fn at(offset: u64, source_len: u64) -> Option<Self> {
        Self::new(offset, offset, source_len)
    }

    pub fn start(self) -> u64 {
        self.start
    }

    pub fn end(self) -> u64 {
        self.end
    }

    pub fn len(self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether `offset` lies inside the range, half-open.
    pub fn contains(self, offset: u64) -> bool {
        self.start <= offset && offset < self.end
    }

    /// The smallest range covering both inputs. Inputs must be ranges over
    /// the same source.
    pub fn cover(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl Display for ByteRange {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_construction() {
        assert!(ByteRange::new(0, 0, 0).is_some());
        assert!(ByteRange::new(2, 5, 5).is_some());
        assert!(ByteRange::new(3, 2, 5).is_none());
        assert!(ByteRange::new(3, 6, 5).is_none());
        assert!(ByteRange::new(0, 1, 0).is_none());
    }

    #[test]
    fn cover_spans_both() {
        let a = ByteRange::new(1, 3, 10).unwrap();
        let b = ByteRange::new(2, 8, 10).unwrap();
        assert_eq!(a.cover(b), ByteRange::new(1, 8, 10).unwrap());
    }
}
