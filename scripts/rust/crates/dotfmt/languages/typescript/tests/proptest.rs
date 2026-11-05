#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_typescript::analyze::{self, Options};
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::format;
use proptest::prelude::*;

fn reserved(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "of"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_input_never_panics_and_is_idempotent(input in ".{0,240}") {
        let config = Config::default();
        let first = format(Path::new("a.ts"), &input, &config, Dialect::Auto);
        if let Ok(first) = first {
            let second = format(Path::new("a.ts"), &first, &config, Dialect::Auto)
                .expect("formatted output must parse");
            prop_assert_eq!(first, second);
        }
    }

    #[test]
    fn generated_statements_are_idempotent(
        name in "[A-Za-z_][A-Za-z0-9_]{0,8}",
        value in any::<i64>(),
        property in "[A-Za-z_][A-Za-z0-9_]{0,8}",
        text in "[A-Za-z0-9 _-]{0,20}",
    ) {
        prop_assume!(!reserved(&name) && !reserved(&property));
        let input = format!(
            "const {name} = {value};\nconst object = {{ {property}: \"{text}\", nested: [{value}, {name}] }};\nconsole.log(object, {name});\n"
        );
        let config = Config::default();
        let first = format(Path::new("a.ts"), &input, &config, Dialect::Auto).unwrap();
        let second = format(Path::new("a.ts"), &first, &config, Dialect::Auto).unwrap();
        prop_assert_eq!(first, second);
    }

    #[test]
    fn generated_tsx_is_idempotent(
        name in "[A-Za-z_][A-Za-z0-9_]{0,8}",
        text in "[A-Za-z0-9 _-]{0,20}",
    ) {
        prop_assume!(!reserved(&name));
        let input = format!(
            "const {name} = () => <section className=\"box\"><h1>{text}</h1><p>{{1 + 2}}</p></section>;\nexport {{ {name} }};\n"
        );
        let config = Config::default();
        let first = format(Path::new("a.tsx"), &input, &config, Dialect::Auto).unwrap();
        let second = format(Path::new("a.tsx"), &first, &config, Dialect::Auto).unwrap();
        prop_assert_eq!(first, second);
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn arbitrary_analysis_never_panics(input in ".{0,240}") {
        let _ = analyze::analyze(
            Path::new("a.ts"),
            &input,
            &Config::default(),
            Dialect::Auto,
            &Options::default(),
        );
    }
}
