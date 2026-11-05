use std::collections::HashSet;
use std::sync::LazyLock;

use rustywind_core::pattern_sorter::PatternSorter;

use crate::order::OrderTable;

static RUST_FALLBACK: LazyLock<PatternSorter> = LazyLock::new(PatternSorter::new);

/// Splits a class list on whitespace that is not escaped or inside `[]`, `()`,
/// `{}`, or quotes.
///
/// The class strings are returned exactly as written; only separators are
/// consumed. This keeps arbitrary values such as `bg-[url('/a b.svg')]` intact.
pub fn split_classes(list: &str) -> Vec<&str> {
    let mut classes = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, character) in list.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            start.get_or_insert(offset);
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                start.get_or_insert(offset);
                quote = Some(character);
            }
            '[' | '(' | '{' => {
                start.get_or_insert(offset);
                depth += 1;
            }
            ']' | ')' | '}' => {
                start.get_or_insert(offset);
                depth = depth.saturating_sub(1);
            }
            _ if character.is_whitespace() && depth == 0 => {
                if let Some(start) = start.take() {
                    classes.push(&list[start..offset]);
                }
            }
            _ => {
                start.get_or_insert(offset);
            }
        }
    }
    if let Some(start) = start {
        classes.push(&list[start..]);
    }
    classes
}

/// Sorts one class list, keeping unknown classes first and `...` last.
///
/// When the table is empty the Rust pattern sorter is used as a fallback.
/// Deduplication only removes repeated known classes; duplicate unknown
/// classes are preserved so custom classes do not disappear.
pub fn sort_class_list(list: &str, table: &OrderTable, dedupe: bool) -> String {
    sort_tokens(&split_classes(list), table, dedupe)
}

pub(crate) fn sort_tokens(tokens: &[&str], table: &OrderTable, dedupe: bool) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    if table.is_empty() {
        // rustywind 0.7 slices class tokens by byte index and panics on
        // non-ASCII input. Keep custom classes untouched; for ASCII, contain
        // any other parser panic rather than taking down a formatter.
        if tokens.iter().any(|class| !class.is_ascii()) {
            return tokens.join(" ");
        }
        let sorted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rustywind_core::pattern_sorter::sort_classes(tokens)
        }))
        .unwrap_or_else(|_| tokens.to_vec());
        return join_fallback(sorted, dedupe);
    }
    let mut ordered: Vec<(&str, Option<i64>)> = tokens
        .iter()
        .map(|class| (*class, table.order(class).flatten()))
        .collect();
    ordered.sort_by(|(a, a_order), (b, b_order)| {
        let (a_ellipsis, b_ellipsis) = (is_ellipsis(a), is_ellipsis(b));
        if a_ellipsis != b_ellipsis {
            return a_ellipsis.cmp(&b_ellipsis);
        }
        if a_ellipsis {
            return std::cmp::Ordering::Equal;
        }
        match (a_order, b_order) {
            (Some(a), Some(b)) => a.cmp(b),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    let mut seen = HashSet::new();
    let mut output = Vec::with_capacity(ordered.len());
    for (class, order) in ordered {
        if dedupe && order.is_some() && !seen.insert(class) {
            continue;
        }
        output.push(class);
    }
    output.join(" ")
}

fn join_fallback(classes: Vec<&str>, dedupe: bool) -> String {
    if !dedupe {
        return classes.join(" ");
    }
    let mut seen = HashSet::new();
    classes
        .into_iter()
        .filter(|class| {
            if RUST_FALLBACK.get_sort_key(class).is_some() {
                seen.insert(*class)
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_known(table: &OrderTable, class: &str) -> bool {
    if table.is_empty() {
        RUST_FALLBACK.get_sort_key(class).is_some()
    } else {
        table.is_known(class)
    }
}

fn is_ellipsis(class: &str) -> bool {
    class == "..." || class == "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> OrderTable {
        OrderTable::from_pairs([
            ("p-4".to_owned(), Some(10)),
            ("m-4".to_owned(), Some(1)),
            ("flex".to_owned(), Some(5)),
        ])
    }

    #[test]
    fn splits_without_breaking_arbitrary_values() {
        assert_eq!(
            split_classes("p-4 bg-[url('/a b.svg')] [mask-type:luminance]  flex"),
            vec![
                "p-4",
                "bg-[url('/a b.svg')]",
                "[mask-type:luminance]",
                "flex"
            ]
        );
    }

    #[test]
    fn splits_quotes_braces_escapes_and_stacked_variants() {
        assert_eq!(
            split_classes(
                r#"dark:hover:bg-[url('/a b.svg')] content-['{hello world}'] before:content-["a b"] foo\ bar p-4/50 -mt-2 !flex"#
            ),
            vec![
                "dark:hover:bg-[url('/a b.svg')]",
                "content-['{hello world}']",
                r#"before:content-["a b"]"#,
                r"foo\ bar",
                "p-4/50",
                "-mt-2",
                "!flex",
            ]
        );
    }

    #[test]
    fn splits_unicode_ellipsis() {
        assert_eq!(
            split_classes("p-4 … flex ..."),
            vec!["p-4", "…", "flex", "..."]
        );
    }

    #[test]
    fn preserves_escaped_whitespace_only_at_bracket_depth_zero() {
        assert_eq!(
            split_classes(r"a\ b c\[d\ e\] f"),
            vec![r"a\ b", r"c\[d\ e\]", "f"]
        );
    }

    #[test]
    fn unknown_classes_sort_first_stably_and_ellipsis_last() {
        assert_eq!(
            sort_class_list("p-4 flex zeta custom m-4 ...", &table(), false),
            "zeta custom m-4 flex p-4 ..."
        );
    }

    #[test]
    fn known_classes_sort_by_project_order() {
        assert_eq!(
            sort_class_list("p-4 m-4 flex", &table(), false),
            "m-4 flex p-4"
        );
    }

    #[test]
    fn dedupes_only_known_classes() {
        let table =
            OrderTable::from_pairs([("p-4".to_owned(), Some(10)), ("custom".to_owned(), None)]);
        assert_eq!(
            sort_class_list("p-4 custom p-4 custom", &table, true),
            "custom custom p-4"
        );
    }

    #[test]
    fn ellipsis_sorts_last_even_with_known_order() {
        let table = OrderTable::from_pairs([
            ("...".to_owned(), Some(0)),
            ("…".to_owned(), Some(1)),
            ("p-4".to_owned(), Some(2)),
        ]);
        assert_eq!(
            sort_class_list("... p-4 … visible", &table, false),
            "visible p-4 ... …"
        );
    }

    #[test]
    fn empty_table_falls_back_to_rust_sorter_without_panicking() {
        let output = sort_class_list("p-4 custom flex custom", &OrderTable::empty(), true);
        assert_eq!(output, "custom custom flex p-4");
    }

    #[test]
    fn sorting_is_idempotent() {
        let input = "custom p-4 flex custom m-4 ... p-4";
        let once = sort_class_list(input, &table(), true);
        assert_eq!(sort_class_list(&once, &table(), true), once);
        let fallback = sort_class_list(input, &OrderTable::empty(), true);
        assert_eq!(
            sort_class_list(&fallback, &OrderTable::empty(), true),
            fallback
        );
    }
}
