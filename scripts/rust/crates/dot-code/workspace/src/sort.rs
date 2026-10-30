//! Port of the VS Code Explorer's file sorter, see `explorerViewer.ts` and `comparers.ts`.

use std::cmp::Ordering;
use std::io;

use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;
use serde::Deserialize;

/// The `explorer.sortOrder*` settings.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Explorer {
    pub sort_order: String,
    pub sort_order_lexicographic_options: String,
    pub sort_order_reverse: bool,
}

pub struct Entry {
    pub name: String,
    /// Milliseconds since the Unix epoch.
    pub modified: u128,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Name,
    Type,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Mixed,
    Upper,
    Lower,
    Unicode,
}

pub struct Sorter {
    order: Order,
    case: Case,
    reverse: bool,
    numeric: CollatorBorrowed<'static>,
    accent: CollatorBorrowed<'static>,
}

impl Sorter {
    pub fn new(explorer: &Explorer, locale: &str) -> io::Result<Self> {
        Ok(Self {
            order: match explorer.sort_order.as_str() {
                "type" => Order::Type,
                "modified" => Order::Modified,
                _ => Order::Name,
            },
            case: match explorer.sort_order_lexicographic_options.as_str() {
                "upper" => Case::Upper,
                "lower" => Case::Lower,
                "unicode" => Case::Unicode,
                _ => Case::Mixed,
            },
            reverse: explorer.sort_order_reverse,
            numeric: collator(locale, Strength::Tertiary)?,
            accent: collator(locale, Strength::Secondary)?,
        })
    }

    pub fn by_modified(&self) -> bool {
        self.order == Order::Modified
    }

    pub fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        let (a, b) = if self.reverse { (b, a) } else { (a, b) };
        match self.order {
            Order::Name => self.by_name(&a.name, &b.name),
            Order::Type => self.by_type(&a.name, &b.name),
            Order::Modified => b
                .modified
                .cmp(&a.modified)
                .then_with(|| self.by_name(&a.name, &b.name)),
        }
    }

    fn by_name(&self, a: &str, b: &str) -> Ordering {
        match self.case {
            Case::Mixed => collate(&self.numeric, a, b),
            Case::Upper => upper_first(a, b).then_with(|| collate(&self.numeric, a, b)),
            Case::Lower => upper_first(b, a).then_with(|| collate(&self.numeric, a, b)),
            Case::Unicode => unicode(a, b),
        }
    }

    fn by_type(&self, a: &str, b: &str) -> Ordering {
        let (x, y) = (extension(a), extension(b));
        if self.case == Case::Unicode {
            return unicode(&x.to_lowercase(), &y.to_lowercase()).then_with(|| unicode(a, b));
        }
        collate(&self.accent, x, y).then_with(|| self.by_name(a, b))
    }
}

fn collator(locale: &str, strength: Strength) -> io::Result<CollatorBorrowed<'static>> {
    let mut preferences = Locale::try_from_str(locale)
        .map(|locale| CollatorPreferences::from(&locale))
        .unwrap_or_default();
    preferences.numeric_ordering = Some(CollationNumericOrdering::True);
    let mut options = CollatorOptions::default();
    options.strength = Some(strength);
    Collator::try_new(preferences, options).map_err(io::Error::other)
}

/// Numeric collation treats `a1` and `a01` as equal, so the shorter name goes first.
fn collate(collator: &CollatorBorrowed, a: &str, b: &str) -> Ordering {
    collator
        .compare(a, b)
        .then_with(|| a.encode_utf16().count().cmp(&b.encode_utf16().count()))
}

fn unicode(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn upper_first(a: &str, b: &str) -> Ordering {
    match (case_of(a), case_of(b)) {
        (Some(true), Some(false)) => Ordering::Less,
        (Some(false), Some(true)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// `Some(true)` when the name starts with an uppercase letter, `Some(false)` for lowercase.
fn case_of(name: &str) -> Option<bool> {
    let first = name.chars().next()?;
    if !first.to_lowercase().eq([first]) {
        Some(true)
    } else if !first.to_uppercase().eq([first]) {
        Some(false)
    } else {
        None
    }
}

/// Dotfiles such as `.eslintrc.json` count as having no extension.
fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !stem.starts_with('.') => extension,
        _ => "",
    }
}
