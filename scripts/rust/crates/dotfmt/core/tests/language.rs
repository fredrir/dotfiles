#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::language::Language;

#[test]
fn language_aliases_and_default_stdin_paths_agree_with_detection() {
    assert_eq!(Language::parse("md"), Ok(Language::Markdown));
    for language in Language::ALL {
        assert_eq!(Language::parse(language.name()), Ok(language));
        assert_eq!(Language::for_path(language.default_stdin()), Some(language));
        for alias in language.aliases() {
            assert_eq!(Language::parse(alias), Ok(language));
        }
    }
}

#[test]
fn native_dialects_and_case_insensitive_extensions_are_detected() {
    for (path, language) in [
        ("settings.DOTFILE", Language::Conf),
        ("settings.jsonc", Language::Json),
        ("settings.hujson", Language::Json),
        ("settings.jwcc", Language::Json),
        ("script.LUAU", Language::Lua),
        ("README.mdown", Language::Markdown),
        ("README.mkd", Language::Markdown),
    ] {
        assert_eq!(Language::for_path(Path::new(path)), Some(language));
    }
    assert_eq!(Language::for_path(Path::new("config.unknown")), None);
}
