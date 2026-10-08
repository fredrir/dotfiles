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
