/// How a value was quoted in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Bare,
    Single,
    Double,
    Backtick,
}

#[derive(Debug)]
pub(crate) struct Document<'a> {
    pub items: Vec<Item<'a>>,
}

impl Document<'_> {
    pub(crate) fn is_blank(&self) -> bool {
        self.items
            .iter()
            .all(|item| matches!(item, Item::Blank { .. }))
    }
}

#[derive(Debug)]
pub(crate) enum Item<'a> {
    Blank {
        count: usize,
    },
    /// `#` through the end of the line, verbatim.
    Comment {
        text: &'a str,
    },
    Entry(Entry<'a>),
    /// A line the parser could not classify, kept byte-for-byte.
    Verbatim {
        raw: &'a str,
    },
}

#[derive(Debug)]
pub(crate) struct Entry<'a> {
    /// Leading indentation before the key, exactly as written.
    pub lead_ws: &'a str,
    /// The `export` token exactly as written.
    pub export: Option<&'a str>,
    /// Whitespace between `export` and the key, exactly as written.
    pub export_gap: &'a str,
    pub key: &'a str,
    pub value: Value<'a>,
    /// Whitespace between the value and a trailing comment, or the end of the
    /// line when there is no comment.
    pub trailing_gap: &'a str,
    /// `#` through the end of the line, verbatim.
    pub comment: Option<&'a str>,
}

#[derive(Debug)]
pub(crate) enum Value<'a> {
    Bare(&'a str),
    Single(&'a str),
    Double(&'a str),
    Backtick(&'a str),
}

impl<'a> Value<'a> {
    pub(crate) fn raw(&self) -> &'a str {
        match self {
            Self::Bare(raw) | Self::Single(raw) | Self::Double(raw) | Self::Backtick(raw) => raw,
        }
    }

    pub(crate) fn style(&self) -> Style {
        match self {
            Self::Bare(_) => Style::Bare,
            Self::Single(_) => Style::Single,
            Self::Double(_) => Style::Double,
            Self::Backtick(_) => Style::Backtick,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.raw().is_empty()
    }
}
