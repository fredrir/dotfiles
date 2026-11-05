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
        protected: bool,
    },
    Comment {
        text: &'a str,
    },
    Entry(Entry<'a>),
    Verbatim {
        raw: &'a str,
    },
}

#[derive(Debug)]
pub(crate) struct Entry<'a> {
    pub lead_ws: &'a str,
    pub export: Option<&'a str>,
    pub export_gap: &'a str,
    pub key: &'a str,
    pub before_eq_gap: &'a str,
    pub after_eq_gap: &'a str,
    pub value: Value<'a>,
    pub trailing_gap: &'a str,
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
