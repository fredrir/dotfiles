use std::fs;

use crate::config::{Config, Configs, NAME};
use crate::render::Indent;

fn settings(body: &str) -> Result<Config, String> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(NAME);
    fs::write(&path, body).unwrap();
    Config::read(&path)
}

fn configured(body: &str) -> Config {
    settings(body).unwrap_or_else(|message| panic!("{message}"))
}

#[test]
fn the_defaults_are_what_the_shipped_config_says() {
    // `shared/tools/jqfmt.dotfile` and `Config::default` are two copies of one
    // answer, and this is what holds them together.
    let shipped = include_str!("../../../../../../../shared/tools/jqfmt.dotfile");
    let default = Config::default();
    let read = configured(shipped);

    assert_eq!(read.indent, default.indent);
    assert_eq!(read.final_newline, default.final_newline);
    assert_eq!(read.indent, Indent::Spaces(2));
    assert!(read.final_newline);
    assert!(read.warnings.is_empty());
}

#[test]
fn a_config_overrides_only_what_it_names() {
    let config = configured("jqfmt {\n  indent = 4\n}\n");

    assert_eq!(config.indent, Indent::Spaces(4));
    assert!(config.final_newline);
}

#[test]
fn indent_takes_the_three_shapes_jq_takes() {
    assert_eq!(
        configured("jqfmt {\n  indent = -1\n}\n").indent,
        Indent::Tabs
    );
    assert_eq!(
        configured("jqfmt {\n  indent = 0\n}\n").indent,
        Indent::Compact
    );
    assert_eq!(
        configured("jqfmt {\n  indent = 7\n}\n").indent,
        Indent::Spaces(7)
    );
}

#[test]
fn a_mistake_in_the_config_is_reported_at_its_line() {
    let faults = [
        (
            "jqfmt {\n  indnet = 2\n}\n",
            NAME.to_string() + ": line 2: unknown setting: indnet",
        ),
        (
            "jqfmt {\n  indent = wide\n}\n",
            format!("{NAME}: line 2: indent must be a whole number, not wide"),
        ),
        (
            "jqfmt {\n  indent = 8\n}\n",
            format!("{NAME}: line 2: indent must be -1 for a tab, or between 0 and 7, not 8"),
        ),
        (
            "jqfmt {\n  final_newline = maybe\n}\n",
            format!("{NAME}: line 2: final_newline must be true or false, not maybe"),
        ),
        (
            "other {\n  indent = 2\n}\n",
            format!("{NAME}: line 1: unknown block: other"),
        ),
        (
            "indent = 2\n",
            format!("{NAME}: line 1: entry outside a block"),
        ),
    ];
    for (body, expected) in faults {
        let error = settings(body).expect_err(body);
        assert!(
            error.ends_with(&expected),
            "{error} should end with {expected}"
        );
    }
}

#[test]
fn the_keys_a_dotfile_formatter_takes_are_named_and_ignored() {
    // `dotfmt.dotfile` carries these, so a config copied from it would otherwise
    // fail with a setting nobody meant to write.
    let config = configured("jqfmt {\n  align = true\n  align_max = 24\n  blank_lines = 1\n}\n");

    assert_eq!(config.indent, Indent::Spaces(2));
    assert_eq!(config.warnings.len(), 3);
    assert!(
        config.warnings[0].contains("align"),
        "{:?}",
        config.warnings
    );
    assert!(
        config
            .warnings
            .iter()
            .all(|warning| warning.starts_with("warning:")),
        "{:?}",
        config.warnings
    );
}

#[test]
fn the_nearest_config_above_the_target_is_the_one_that_governs() {
    let root = tempfile::tempdir().unwrap();
    let deep = root.path().join("a/b");
    fs::create_dir_all(&deep).unwrap();
    fs::write(root.path().join(NAME), "jqfmt {\n  indent = 6\n}\n").unwrap();

    assert_eq!(Config::resolve(&deep).unwrap().indent, Indent::Spaces(6));
}

#[test]
fn a_config_beside_a_directory_beats_the_one_above_it_for_the_files_below() {
    let root = tempfile::tempdir().unwrap();
    let deep = root.path().join("a/b");
    fs::create_dir_all(&deep).unwrap();
    fs::write(root.path().join(NAME), "jqfmt {\n  indent = 6\n}\n").unwrap();
    fs::write(deep.join(NAME), "jqfmt {\n  indent = 3\n}\n").unwrap();
    let configs = Configs::new();

    let above = configs.for_file(&root.path().join("top.json")).unwrap();
    let below = configs.for_file(&deep.join("under.json")).unwrap();

    assert_eq!(above.indent, Indent::Spaces(6));
    assert_eq!(below.indent, Indent::Spaces(3));
}
