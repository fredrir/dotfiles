#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_ignore::config::Config;
use dotfmt_ignore::dialect::Dialect;
use dotfmt_ignore::{format, format_with_dialect};

#[test]
fn every_variant_has_a_name_and_aliases() {
    assert_eq!(
        Dialect::ALL.map(Dialect::name),
        ["auto", "git", "ignore", "prettier", "vscode", "attributes"]
    );
    assert_eq!(Dialect::Auto.aliases(), &[] as &[&str]);
    assert_eq!(Dialect::Git.aliases(), &["gitignore"]);
    assert_eq!(Dialect::Ignore.aliases(), &["ripgrep"]);
    assert_eq!(Dialect::Prettier.aliases(), &["prettierignore"]);
    assert_eq!(Dialect::Vscode.aliases(), &["vscodeignore"]);
    assert_eq!(Dialect::Attributes.aliases(), &["gitattributes", "attr"]);
}

#[test]
fn names_and_aliases_parse_to_their_variant() {
    for dialect in Dialect::ALL {
        assert_eq!(Dialect::parse(dialect.name(), false).unwrap(), dialect);
        assert_eq!(dialect.name().parse::<Dialect>().unwrap(), dialect);
        for alias in dialect.aliases() {
            assert_eq!(Dialect::parse(alias, false).unwrap(), dialect);
        }
    }
}

#[test]
fn parsing_is_case_sensitive_unless_requested_otherwise() {
    assert_eq!(Dialect::parse("Git", true).unwrap(), Dialect::Git);
    assert_eq!(Dialect::parse("GITIGNORE", true).unwrap(), Dialect::Git);
    assert!(Dialect::parse("Git", false).is_err());
    assert!(Dialect::parse("", false).is_err());
    assert!(Dialect::parse("bogus", false).is_err());
}

#[test]
fn resolve_prefers_the_explicit_then_configured_then_detected_dialect() {
    assert_eq!(
        Dialect::Attributes.resolve(Dialect::Git, Dialect::Prettier),
        Dialect::Attributes
    );
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Git, Dialect::Prettier),
        Dialect::Git
    );
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Auto, Dialect::Prettier),
        Dialect::Prettier
    );
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Auto, Dialect::Auto),
        Dialect::Auto
    );
}

#[test]
fn filenames_are_detected() {
    for (path, expected) in [
        (".gitignore", Dialect::Git),
        ("nested/.gitignore", Dialect::Git),
        ("project.gitignore", Dialect::Git),
        (".git/info/exclude", Dialect::Git),
        (".git/info/attributes", Dialect::Attributes),
        (".ignore", Dialect::Ignore),
        ("ignore", Dialect::Git),
        ("notes.ignore", Dialect::Ignore),
        (".prettierignore", Dialect::Prettier),
        (".vscodeignore", Dialect::Vscode),
        (".gitattributes", Dialect::Attributes),
        ("project.gitattributes", Dialect::Attributes),
        ("notes.txt", Dialect::Git),
        ("config.unknown", Dialect::Git),
    ] {
        assert_eq!(Dialect::detect(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn git_info_attributes_is_detected_as_attributes() {
    let path = Path::new("repo/.git/info/attributes");
    assert_eq!(Dialect::detect(path), Dialect::Attributes);
    assert_eq!(
        format(path, b"*.c\ttext\n", &Config::default()).unwrap(),
        "*.c text"
    );
}

#[test]
fn format_uses_the_filename_when_the_setting_is_auto() {
    let config = Config::default();
    assert_eq!(
        format(Path::new("repo/.gitignore"), b"a\t\n", &config).unwrap(),
        "a\t"
    );
    assert_eq!(
        format(Path::new("repo/.prettierignore"), b"a\t\n", &config).unwrap(),
        "a"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_basenames_keep_their_suffix_dialect() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let attributes = Path::new(OsStr::from_bytes(b"caf\xe9.gitattributes"));
    assert_eq!(Dialect::detect(attributes), Dialect::Attributes);
    let ignore = Path::new(OsStr::from_bytes(b"caf\xe9.ignore"));
    assert_eq!(Dialect::detect(ignore), Dialect::Ignore);
}

#[test]
fn a_configured_dialect_overrides_the_filename() {
    let config = Config {
        dialect: Dialect::Prettier,
        ..Config::default()
    };
    assert_eq!(
        format(Path::new("repo/.gitignore"), b"a\t\n", &config).unwrap(),
        "a"
    );
}

#[test]
fn an_explicit_argument_overrides_both_config_and_filename() {
    let config = Config {
        dialect: Dialect::Prettier,
        ..Config::default()
    };
    assert_eq!(
        format_with_dialect("a\t\n", &config, Dialect::Git).unwrap(),
        "a\t"
    );
    assert_eq!(
        format_with_dialect("a\t\n", &Config::default(), Dialect::Auto).unwrap(),
        "a\t"
    );
    let attributes = Config {
        dialect: Dialect::Attributes,
        ..Config::default()
    };
    assert_eq!(
        format_with_dialect("*.c\ttext\n", &attributes, Dialect::Auto).unwrap(),
        "*.c text"
    );
}
