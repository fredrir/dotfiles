#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use dotfmt_css::config::Config;
use dotfmt_css::dialect::Dialect;
use dotfmt_css::format;
use dotfmt_tailwind::{OrderTable, Sorter};
use proptest::prelude::*;

const CLASSES: &[&str] = &[
    "m-2",
    "flex",
    "p-4",
    "block",
    "hover:bg-red-500",
    "md:flex",
    "bg-[url(\"/a b.svg\")]",
    "content-[\";\"]",
    "grid-cols-[repeat(2,minmax(0,1fr))]",
    "unknown",
];

fn sorter() -> Sorter {
    let classes = CLASSES
        .iter()
        .enumerate()
        .map(|(index, class)| ((*class).to_owned(), Some(index as i64)));
    Sorter::new(
        Arc::new(OrderTable::from_pairs(classes)),
        dotfmt_tailwind::Config::default(),
    )
}

fn tailwind_config() -> Config {
    let mut config = Config {
        tailwind_sorter: Some(sorter()),
        ..Config::default()
    };
    config.tailwind.mode = dotfmt_tailwind::Mode::On;
    config
}

fn windows_config() -> Config {
    Config {
        line_endings: dotfmt_css::config::LineEndings::Windows,
        final_newline: false,
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_input_never_panics_and_is_idempotent(input in any::<String>()) {
        for config in [Config::default(), windows_config()] {
            for dialect in Dialect::ALL {
                if let Ok(once) = format(Path::new("input.css"), &input, &config, dialect) {
                    let twice = format(Path::new("input.css"), &once, &config, dialect)
                        .expect("formatted output must parse again");
                    prop_assert_eq!(once, twice, "dialect {:?}, input {:?}", dialect, input);
                }
            }
        }
    }

    #[test]
    fn generated_apply_rules_are_idempotent(
        rules in prop::collection::vec(
            (
                prop::collection::vec(prop::sample::select(CLASSES), 1..6),
                any::<bool>(),
                any::<bool>(),
            ),
            1..4,
        )
    ) {
        let document = rules
            .into_iter()
            .map(|(classes, important, comment)| {
                let mut list = classes.join(" ");
                if important {
                    list.push_str(" !important");
                }
                if comment {
                    list.push_str("/* note */");
                }
                format!("a{{@apply {list};}}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let config = tailwind_config();
        let once = format(Path::new("input.css"), &document, &config, Dialect::Css).unwrap();
        let twice = format(Path::new("input.css"), &once, &config, Dialect::Css).unwrap();
        prop_assert_eq!(once, twice, "document {:?}", document);
    }
}
