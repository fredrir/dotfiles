use super::*;

#[test]
fn a_target_and_fold_commands_parse() {
    let config = Config::parse(
        r#"
output = ".cache/zsh/build"
ambient = ["HOME"]

[[target]]
name = "zshrc"
source = "shared/zsh/01-init.zsh"
env = ["shared/zsh/00-global.zsh"]

[fold]
commands = ["brew --prefix", "git  version"]
"#,
    )
    .unwrap();
    assert_eq!(config.targets[0].name, "zshrc");
    assert_eq!(config.fold.timeout_ms, 5000);
    assert_eq!(
        config.fold_commands(),
        vec![
            vec!["brew".to_string(), "--prefix".into()],
            vec!["git".into(), "version".into()]
        ]
    );
}

#[test]
fn unknown_keys_and_empty_commands_are_rejected() {
    assert!(Config::parse("output = \"x\"\nextra = 1\n").is_err());
    assert!(Config::parse("output = \"x\"\n[fold]\ncommands = [\" \"]\n").is_err());
}

#[test]
fn system_settings_parse_with_defaults() {
    let config = Config::parse(
        r#"
output = "x"

[[target]]
name = "zshrc"
source = "rc.zsh"
profile = "profile.zsh"
system = true

[system]
ambient = ["HOME"]
"#,
    )
    .unwrap();
    assert!(config.targets[0].system);
    assert_eq!(config.system.ambient, vec!["HOME".to_string()]);
    assert_eq!(config.system.path_helper_root, "");
    let default = if cfg!(target_os = "macos") {
        "/etc"
    } else {
        "/etc/zsh"
    };
    assert_eq!(config.system.dir, PathBuf::from(default));
}

#[test]
fn a_profile_needs_its_target_to_compile_the_system_files() {
    let profile = "output = \"x\"\n[[target]]\nname = \"a\"\nsource = \"a\"\nprofile = \"p\"\n";
    assert!(Config::parse(profile).is_err());
    let twice = "output = \"x\"\n[[target]]\nname = \"a\"\nsource = \"a\"\nsystem = true\n[[target]]\nname = \"b\"\nsource = \"b\"\nsystem = true\n";
    assert!(Config::parse(twice).is_err());
}

#[test]
fn nothing_is_deferred_by_default() {
    let config = Config::parse("output = \"x\"\n").unwrap();
    assert!(!config.defer.compinit);
    assert!(config.defer.plugins.is_empty() && config.defer.files.is_empty());
    let config = Config::parse(
        r#"
output = "x"

[defer]
compinit = true
plugins = ["git"]
evals = ["*-completion"]
files = ["zsh/70-*.zsh"]
"#,
    )
    .unwrap();
    assert!(config.defer.compinit);
    assert_eq!(config.defer.evals, vec!["*-completion".to_string()]);
}

#[test]
fn bad_defer_patterns_and_unknown_keys_are_rejected() {
    assert!(Config::parse("output = \"x\"\n[defer]\nplugins = [\"[\"]\n").is_err());
    assert!(Config::parse("output = \"x\"\n[defer]\nlibs = [\"x\"]\n").is_err());
}

#[test]
fn a_missing_fold_table_keeps_the_default_timeout() {
    let config = Config::parse("output = \"x\"\n").unwrap();
    assert_eq!(config.fold.timeout_ms, 5000);
}
