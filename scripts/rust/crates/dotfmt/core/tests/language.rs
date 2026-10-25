#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::language::Language;

#[test]
fn language_aliases_and_default_stdin_paths_agree_with_detection() {
    assert_eq!(Language::parse("md"), Ok(Language::Markdown));
    assert_eq!(Language::parse("yml"), Ok(Language::Yaml));
    assert_eq!(Language::parse("tex"), Ok(Language::Latex));
    assert_eq!(Language::parse("bibtex"), Ok(Language::Bib));
    assert_eq!(Language::parse("biblatex"), Ok(Language::Bib));
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
        ("refs.bib", Language::Bib),
        ("refs.BIB", Language::Bib),
        ("settings.DOTFILE", Language::Conf),
        ("settings.jsonc", Language::Json),
        ("settings.hujson", Language::Json),
        ("settings.jwcc", Language::Json),
        ("events.jsonl", Language::Json),
        ("events.NDJSON", Language::Json),
        ("events.jsonlines", Language::Json),
        ("paper.tex", Language::Latex),
        ("paper.STY", Language::Latex),
        ("package.cls", Language::Latex),
        ("package.dtx", Language::Latex),
        ("context.ctx", Language::Latex),
        ("script.LUAU", Language::Lua),
        ("README.mdown", Language::Markdown),
        ("README.mkd", Language::Markdown),
        ("config.toml", Language::Toml),
        ("config.TOML", Language::Toml),
        ("config.yaml", Language::Yaml),
        ("config.YML", Language::Yaml),
    ] {
        assert_eq!(Language::for_path(Path::new(path)), Some(language));
    }
    assert_eq!(Language::for_path(Path::new("config.unknown")), None);
}
