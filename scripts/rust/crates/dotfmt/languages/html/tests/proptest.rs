#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use dotfmt_html::config::LineEndings;
use dotfmt_html::{Config, Dialect, format};
use dotfmt_tailwind::{OrderTable, Sorter};
use proptest::prelude::*;

fn configs() -> Vec<(Config, Dialect)> {
    let table = OrderTable::from_pairs([
        ("p-4".to_owned(), Some(10)),
        ("flex".to_owned(), Some(5)),
        ("m-4".to_owned(), Some(1)),
        ("block".to_owned(), Some(2)),
    ]);
    let sorter = Sorter::new(Arc::new(table), dotfmt_tailwind::Config::default());
    let tailwind = Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::On,
            ..dotfmt_tailwind::Config::default()
        },
        tailwind_sorter: Some(sorter),
        ..Config::default()
    };
    vec![
        (Config::default(), Dialect::Html),
        (tailwind, Dialect::Html),
        (
            Config {
                quotes: markup_fmt::config::Quotes::Single,
                final_newline: false,
                ..Config::default()
            },
            Dialect::Vue,
        ),
        (
            Config {
                line_endings: LineEndings::Windows,
                ..Config::default()
            },
            Dialect::Svelte,
        ),
        (Config::default(), Dialect::Astro),
    ]
}

fn arbitrary_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(any::<char>(), 0..300).prop_map(String::from_iter)
}

fn markup_fragments() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("<div>".to_owned()),
            Just("</div>".to_owned()),
            Just("<span class=\"p-4 flex\">".to_owned()),
            Just("</span>".to_owned()),
            Just("<p className=\"block m-4\">".to_owned()),
            Just("</p>".to_owned()),
            Just("<script>const a=1</script>".to_owned()),
            Just("<style>.a{color:red}</style>".to_owned()),
            Just("<!-- <div class=\"p-4 flex\"> -->".to_owned()),
            Just("text &amp; caf\u{e9}".to_owned()),
            Just("\n".to_owned()),
        ],
        0..12,
    )
    .prop_map(|fragments| fragments.concat())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_input_never_panics(input in arbitrary_text()) {
        for (config, dialect) in configs() {
            let _ = format(Path::new("x.html"), &input, &config, dialect);
        }
    }

    #[test]
    fn successful_formatting_is_idempotent(input in markup_fragments()) {
        for (config, dialect) in configs() {
            let path = Path::new("x.html");
            if let Ok(output) = format(path, &input, &config, dialect) {
                let second = format(path, &output, &config, dialect)
                    .unwrap_or_else(|error| panic!("second pass failed: {error}\ninput: {input:?}\noutput: {output:?}"));
                prop_assert_eq!(second, output, "input: {:?}", input);
            }
        }
    }
}
