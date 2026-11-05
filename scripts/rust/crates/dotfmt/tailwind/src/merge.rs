use std::collections::HashSet;

use crate::order::OrderTable;
use crate::sorter::is_known;

const FAMILIES: &[&str] = &[
    "border-spacing-x",
    "border-spacing-y",
    "border-spacing",
    "grid-cols",
    "grid-rows",
    "line-clamp",
    "max-h",
    "max-w",
    "min-h",
    "min-w",
    "outline-offset",
    "overflow-x",
    "overflow-y",
    "rounded-br",
    "rounded-tl",
    "rounded-tr",
    "rounded-bl",
    "rounded-t",
    "rounded-r",
    "rounded-b",
    "rounded-l",
    "rounded-s",
    "rounded-e",
    "rounded-ss",
    "rounded-se",
    "rounded-es",
    "rounded-ee",
    "underline-offset",
    "aspect",
    "basis",
    "bottom",
    "col-end",
    "col-span",
    "col-start",
    "duration",
    "delay",
    "gap-x",
    "gap-y",
    "grow",
    "h",
    "inset-x",
    "inset-y",
    "leading",
    "left",
    "opacity",
    "order",
    "overflow",
    "p",
    "m",
    "px",
    "py",
    "pt",
    "pr",
    "pb",
    "pl",
    "ps",
    "pe",
    "mx",
    "my",
    "mt",
    "mr",
    "mb",
    "ml",
    "ms",
    "me",
    "right",
    "rotate",
    "row-end",
    "row-span",
    "row-start",
    "rounded",
    "scale-x",
    "scale-y",
    "scale",
    "shrink",
    "size",
    "skew-x",
    "skew-y",
    "start",
    "top",
    "tracking",
    "translate-x",
    "translate-y",
    "w",
    "z",
    "end",
    "gap",
    "inset",
];

pub(crate) fn merge_tokens<'a>(classes: &[&'a str], table: &OrderTable) -> Vec<&'a str> {
    if classes.len() < 2 {
        return classes.to_vec();
    }
    let mut winners: HashSet<(&str, &'static str, bool)> = HashSet::new();
    let mut keep = vec![true; classes.len()];
    for (index, class) in classes.iter().enumerate().rev() {
        let Some(key) = conflict_key(class, table) else {
            continue;
        };
        if !winners.insert(key) {
            keep[index] = false;
        }
    }
    classes
        .iter()
        .enumerate()
        .filter_map(|(index, class)| keep[index].then_some(*class))
        .collect()
}

fn conflict_key<'a>(class: &'a str, table: &OrderTable) -> Option<(&'a str, &'static str, bool)> {
    if !is_known(table, class) || has_arbitrary_syntax(class) {
        return None;
    }
    let (important, normalized) = strip_importance(class);
    let (variant, utility) = split_variant(normalized);
    let (utility_important, utility) = strip_importance(utility);
    let utility = utility.strip_prefix('-').unwrap_or(utility);
    let family = utility_family(utility)?;
    Some((variant, family, important || utility_important))
}

fn has_arbitrary_syntax(class: &str) -> bool {
    class
        .chars()
        .any(|character| matches!(character, '[' | ']' | '(' | ')'))
}

fn strip_importance(class: &str) -> (bool, &str) {
    let mut important = false;
    let mut class = class;
    if let Some(rest) = class.strip_prefix('!') {
        important = true;
        class = rest;
    }
    if let Some(rest) = class.strip_suffix('!') {
        important = true;
        class = rest;
    }
    (important, class)
}

fn split_variant(class: &str) -> (&str, &str) {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut last_colon = None;
    for (offset, character) in class.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => last_colon = Some(offset),
            _ => {}
        }
    }
    match last_colon {
        Some(offset) => (&class[..offset], &class[offset + 1..]),
        None => ("", class),
    }
}

fn utility_family(utility: &str) -> Option<&'static str> {
    FAMILIES
        .iter()
        .filter(|family| {
            utility
                .strip_prefix(*family)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
        })
        .max_by_key(|family| family.len())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> OrderTable {
        OrderTable::from_pairs([
            ("p-4".to_owned(), Some(1)),
            ("p-2".to_owned(), Some(2)),
            ("m-4".to_owned(), Some(3)),
            ("mt-2".to_owned(), Some(4)),
            ("-mt-2".to_owned(), Some(5)),
            ("hover:p-4".to_owned(), Some(6)),
            ("hover:p-2".to_owned(), Some(7)),
            ("focus:p-4".to_owned(), Some(8)),
            ("!p-4".to_owned(), Some(9)),
            ("!p-2".to_owned(), Some(10)),
            ("gap-4".to_owned(), Some(11)),
            ("gap-x-2".to_owned(), Some(12)),
            ("rounded-lg".to_owned(), Some(13)),
            ("rounded-tl-lg".to_owned(), Some(14)),
            ("custom".to_owned(), None),
        ])
    }

    #[test]
    fn merges_same_family_and_variant_keeping_rightmost() {
        assert_eq!(merge_tokens(&["p-4", "p-2"], &table()), vec!["p-2"]);
        assert_eq!(
            merge_tokens(&["hover:p-4", "hover:p-2"], &table()),
            vec!["hover:p-2"]
        );
    }

    #[test]
    fn does_not_merge_different_families_variants_or_importance() {
        let table = table();
        assert_eq!(merge_tokens(&["p-4", "m-4"], &table), vec!["p-4", "m-4"]);
        assert_eq!(
            merge_tokens(&["hover:p-4", "focus:p-4", "p-2"], &table),
            vec!["hover:p-4", "focus:p-4", "p-2"]
        );
        assert_eq!(merge_tokens(&["!p-4", "p-2"], &table), vec!["!p-4", "p-2"]);
        assert_eq!(merge_tokens(&["!p-4", "!p-2"], &table), vec!["!p-2"]);
    }

    #[test]
    fn negative_and_positive_spacing_conflict() {
        assert_eq!(merge_tokens(&["-mt-2", "mt-2"], &table()), vec!["mt-2"]);
    }

    #[test]
    fn longest_family_prefix_wins() {
        assert_eq!(
            merge_tokens(&["gap-x-2", "gap-4"], &table()),
            vec!["gap-x-2", "gap-4"]
        );
        assert_eq!(
            merge_tokens(&["rounded-lg", "rounded-tl-lg"], &table()),
            vec!["rounded-lg", "rounded-tl-lg"]
        );
    }

    #[test]
    fn skips_arbitrary_values_unknown_classes_and_duplicates() {
        let table = table();
        assert_eq!(
            merge_tokens(&["p-[10px]", "p-2"], &table),
            vec!["p-[10px]", "p-2"]
        );
        assert_eq!(
            merge_tokens(&["p-4", "p-unknown"], &table),
            vec!["p-4", "p-unknown"]
        );
        assert_eq!(
            merge_tokens(&["custom", "custom"], &table),
            vec!["custom", "custom"]
        );
    }

    #[test]
    fn merges_with_rust_fallback_known_classes() {
        assert_eq!(
            merge_tokens(&["p-4", "p-2", "custom", "custom"], &OrderTable::empty()),
            vec!["p-2", "custom", "custom"]
        );
    }

    #[test]
    fn variants_with_utility_level_importance_only_merge_when_both_are_important() {
        let table = OrderTable::from_pairs([
            ("hover:!p-4".to_owned(), Some(1)),
            ("hover:!p-2".to_owned(), Some(2)),
            ("hover:p-2".to_owned(), Some(3)),
        ]);
        assert_eq!(
            merge_tokens(&["hover:!p-4", "hover:!p-2"], &table),
            vec!["hover:!p-2"]
        );
        assert_eq!(
            merge_tokens(&["hover:!p-4", "hover:p-2"], &table),
            vec!["hover:!p-4", "hover:p-2"]
        );
    }

    #[test]
    fn border_spacing_axes_do_not_merge_with_both_axes() {
        let table = OrderTable::from_pairs([
            ("border-spacing-2".to_owned(), Some(1)),
            ("border-spacing-x-2".to_owned(), Some(2)),
        ]);
        assert_eq!(
            merge_tokens(&["border-spacing-2", "border-spacing-x-2"], &table),
            vec!["border-spacing-2", "border-spacing-x-2"]
        );
    }

    #[test]
    fn skips_arbitrary_variants() {
        let table = OrderTable::from_pairs([
            ("[&:hover]:p-4".to_owned(), Some(1)),
            ("[&:hover]:p-2".to_owned(), Some(2)),
        ]);
        assert_eq!(
            merge_tokens(&["[&:hover]:p-4", "[&:hover]:p-2"], &table),
            vec!["[&:hover]:p-4", "[&:hover]:p-2"]
        );
    }
}
