#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use dotfmt_core::language::Language;

#[test]
fn language_aliases_and_default_stdin_paths_agree_with_detection() {
    assert_eq!(Language::parse("md"), Ok(Language::Markdown));
    assert_eq!(Language::parse("yml"), Ok(Language::Yaml));
    assert_eq!(Language::parse("tex"), Ok(Language::Latex));
    assert_eq!(Language::parse("bibtex"), Ok(Language::Bib));
    assert_eq!(Language::parse("biblatex"), Ok(Language::Bib));
    assert_eq!(Language::parse("dotenv"), Ok(Language::Env));
    assert_eq!(Language::parse("gitignore"), Ok(Language::Ignore));
    assert_eq!(Language::parse("tsx"), Ok(Language::TypeScript));
    assert_eq!(Language::parse("jsx"), Ok(Language::Javascript));
    assert_eq!(Language::parse("scss"), Ok(Language::Css));
    assert_eq!(Language::parse("vue"), Ok(Language::Html));
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
        (".gitconfig", Language::Ini),
        ("work/.gitconfig", Language::Ini),
        (".gitmodules", Language::Ini),
        (".editorconfig", Language::Ini),
        (".git/config", Language::Ini),
        ("home/.config/git/config", Language::Ini),
        ("unit.service", Language::Ini),
        ("unit.SERVICE", Language::Ini),
        ("unit.timer", Language::Ini),
        ("unit.path", Language::Ini),
        ("ssh_config", Language::Ini),
        ("sshd_config", Language::Ini),
        ("home/.ssh/config", Language::Ini),
        ("home/.ssh/config.d/50-servers", Language::Ini),
        (".env", Language::Env),
        (".env.local", Language::Env),
        ("work/.env.production", Language::Env),
        ("performance.env", Language::Env),
        ("env", Language::Env),
        (".gitignore", Language::Ignore),
        ("work/.gitignore", Language::Ignore),
        (".ignore", Language::Ignore),
        ("ignore", Language::Ignore),
        (".prettierignore", Language::Ignore),
        (".vscodeignore", Language::Ignore),
        (".gitattributes", Language::Ignore),
        (".git/info/exclude", Language::Ignore),
        (".git/info/attributes", Language::Ignore),
        ("Info.plist", Language::Plist),
        ("com.example.plist", Language::Plist),
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
        ("component.tsx", Language::TypeScript),
        ("module.mts", Language::TypeScript),
        ("script.JSX", Language::Javascript),
        ("style.scss", Language::Css),
        ("style.pcss", Language::Css),
        ("page.html", Language::Html),
        ("page.vue", Language::Html),
    ] {
        assert_eq!(
            Language::for_path(Path::new(path)),
            Some(language),
            "{path}"
        );
    }
    for path in [
        "config.unknown",
        "notes.txt",
        ".envrc",
        ".envrc.local",
        ".environment",
        "notssh/config",
        "reports/config",
        ".ssh/config.d/nested/50-servers",
    ] {
        assert_eq!(Language::for_path(Path::new(path)), None, "{path}");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_basenames_keep_utf8_extension_detection() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"caf\xe9.toml"));
    assert_eq!(Language::for_path(path), Some(Language::Toml));
}

#[test]
fn built_in_names_do_not_claim_the_same_path_twice() {
    let mut owners = BTreeMap::new();
    for language in Language::ALL {
        for name in language.extensions() {
            let path = format!("sample.{name}");
            if let Some(previous) = owners.insert(path.clone(), language) {
                panic!(
                    "{path} claimed by {} and {}",
                    previous.name(),
                    language.name()
                );
            }
        }
        for name in language.aliases() {
            if let Some(previous) = owners.insert(format!("alias:{name}"), language) {
                panic!(
                    "alias {name} claimed by {} and {}",
                    previous.name(),
                    language.name()
                );
            }
        }
    }
    // The concrete path table is the real collision test: exact names,
    // prefixes, suffixes and directories are checked through for_path.
    for (path, language) in [
        (".gitconfig", Language::Ini),
        (".gitmodules", Language::Ini),
        (".editorconfig", Language::Ini),
        (".git/config", Language::Ini),
        ("home/.config/git/config", Language::Ini),
        ("ssh_config", Language::Ini),
        ("sshd_config", Language::Ini),
        ("other/ssh/config", Language::Ini),
        ("other/ssh/config.d/50-servers", Language::Ini),
        (".env", Language::Env),
        (".env.local", Language::Env),
        ("env", Language::Env),
        (".gitignore", Language::Ignore),
        (".ignore", Language::Ignore),
        ("ignore", Language::Ignore),
        (".prettierignore", Language::Ignore),
        (".vscodeignore", Language::Ignore),
        (".gitattributes", Language::Ignore),
        (".git/info/exclude", Language::Ignore),
        (".git/info/attributes", Language::Ignore),
        ("Info.plist", Language::Plist),
    ] {
        assert_eq!(
            Language::for_path(Path::new(path)),
            Some(language),
            "{path}"
        );
    }
}
