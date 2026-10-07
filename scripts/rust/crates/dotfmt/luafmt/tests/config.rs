#![forbid(unsafe_code)]

use luafmt::{
    config::{Config, Configs, NAME},
    format,
};
use rayon::prelude::*;
use testkit::tree_pairs;

#[test]
fn shipped_configuration_matches_builtin_defaults() {
    let shipped = include_str!("../../../../../../shared/tools/luafmt.dotfile");
    let root = tree_pairs(&[(NAME, shipped)]);
    let config = Config::read(&root.path().join(NAME)).unwrap();
    let defaults = Config::default();
    assert_eq!(config.dialect, defaults.dialect);
    assert_eq!(
        format!("{:?}", config.style),
        format!("{:?}", defaults.style)
    );
    assert_eq!(config.final_newline, defaults.final_newline);
    assert_eq!(config.verify, defaults.verify);
    assert!(config.files.allows(&root.path().join("a.lua")));
}

#[test]
fn rejects_invalid_settings_with_source_and_line() {
    for setting in [
        "width = 0",
        "width = -1",
        "width = 10001",
        "indent = 0",
        "indent = 17",
        "indent_type = wide",
        "dialect = unknown",
        "quote_style = curly",
        "line_endings = mac",
        "call_parentheses = maybe",
        "collapse_simple_statement = yes",
        "space_after_function_names = yes",
        "block_newline_gaps = yes",
        "sort_requires = yes",
        "verify = yes",
        "final_newline = yes",
        "widht = 80",
    ] {
        let text = format!("luafmt {{\n{setting}\n}}\n");
        let root = tree_pairs(&[(NAME, &text)]);
        let error = Config::read(&root.path().join(NAME)).unwrap_err();
        assert!(
            error.contains("luafmt.dotfile: line 2:"),
            "{setting}: {error}"
        );
    }
    for text in [
        "}",
        "width = 80",
        "unknown {\n}",
        "luafmt {\nluafmt {\n}\n}",
        "luafmt {\nwidth = 80",
    ] {
        let root = tree_pairs(&[(NAME, text)]);
        assert!(Config::read(&root.path().join(NAME)).is_err(), "{text}");
    }
}

#[test]
fn parallel_resolution_honors_nearest_config_and_caches_failures() {
    let root = tree_pairs(&[
        (NAME, "luafmt {\nindent = 4\n}"),
        ("sub/luafmt.dotfile", "luafmt {\nindent = 3\n}"),
        ("bad/luafmt.dotfile", "luafmt {\nindent = -1\n}"),
    ]);
    let configs = Configs::new();
    (0..128).into_par_iter().for_each(|i| {
        let (name, expected) = if i % 2 == 0 {
            ("sub/deep/new.lua", 3)
        } else {
            ("deep/new.lua", 4)
        };
        let config = configs.for_file(&root.path().join(name)).unwrap();
        assert_eq!(config.style.indent_width, expected);
        let formatted = format("if x then f() end", &config).unwrap();
        assert!(formatted.contains(&format!("\n{}f()\n", " ".repeat(expected))));
        assert!(
            configs
                .for_file(&root.path().join("bad/deep/new.lua"))
                .is_err()
        );
    });
}
