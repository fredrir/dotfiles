use dotfmt_tailwind::{OrderTable, sort_class_list, split_classes};
use proptest::prelude::*;

fn table() -> OrderTable {
    OrderTable::from_pairs([
        ("p-4".to_owned(), Some(1)),
        ("m-4".to_owned(), Some(2)),
        ("flex".to_owned(), Some(3)),
        ("hover:p-4".to_owned(), Some(4)),
        ("custom".to_owned(), None),
    ])
}

proptest! {
    #[test]
    fn tokenizer_never_panics(input in ".*") {
        let _ = split_classes(&input);
    }

    #[test]
    fn project_sorter_never_panics(input in ".*") {
        let _ = sort_class_list(&input, &table(), true);
    }

    #[test]
    fn fallback_sorter_never_panics(input in ".*") {
        let _ = sort_class_list(&input, &OrderTable::empty(), true);
    }

    #[test]
    fn sorters_are_idempotent_for_class_lists(
        classes in prop::collection::vec("[A-Za-z0-9_:./!-]{0,30}", 0..20)
    ) {
        let input = classes.join(" ");
        for table in [table(), OrderTable::empty()] {
            let once = sort_class_list(&input, &table, true);
            let twice = sort_class_list(&once, &table, true);
            prop_assert_eq!(once, twice);
        }
    }
}
