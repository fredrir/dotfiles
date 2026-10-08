use std::collections::BTreeMap;

use super::*;

struct Vars(BTreeMap<&'static str, Var>);

impl Env for Vars {
    fn var(&self, name: &str) -> Var {
        self.0.get(name).cloned().unwrap_or(Var::Unknown)
    }
}

fn env() -> Vars {
    Vars(BTreeMap::from([
        ("HOME", Var::Scalar("/home/me".into())),
        ("ZSH", Var::Scalar("/home/me/.oh-my-zsh".into())),
        ("EMPTY", Var::Scalar(String::new())),
        ("GONE", Var::Unset),
        ("plugins", Var::Array(vec!["git".into(), "fzf".into()])),
        ("file", Var::Scalar("/a/b/name.plugin.zsh".into())),
    ]))
}

fn args(word: &str) -> Option<Vec<String>> {
    words(word, &env(), Mode::Args)
}

#[test]
fn quotes_and_parameters_join_into_one_word() {
    assert_eq!(
        args(r#""$ZSH"/lib'/x y'"#),
        Some(vec!["/home/me/.oh-my-zsh/lib/x y".into()])
    );
    assert_eq!(args(r"$'a\tb'"), Some(vec!["a\tb".into()]));
    assert_eq!(args("~/bin"), Some(vec!["/home/me/bin".into()]));
}

#[test]
fn defaults_and_modifiers_apply() {
    assert_eq!(
        scalar("${GONE:-/opt/homebrew}/share", &env()),
        Some("/opt/homebrew/share".into())
    );
    assert_eq!(scalar("${EMPTY-set}", &env()), Some(String::new()));
    assert_eq!(scalar("${EMPTY:-unset}", &env()), Some("unset".into()));
    assert_eq!(scalar("${file:t}", &env()), Some("name.plugin.zsh".into()));
    assert_eq!(scalar("${file:h:t}", &env()), Some("b".into()));
    assert_eq!(scalar("$file:r", &env()), Some("/a/b/name.plugin".into()));
}

#[test]
fn unknown_values_are_not_evaluated() {
    assert_eq!(args("$NEVER_SET"), None);
    assert_eq!(args("${GONE:-$NEVER_SET}"), None);
    assert_eq!(args("$(date)"), None);
    assert_eq!(args("${(k)plugins}"), None);
}

#[test]
fn whole_arrays_expand_to_elements() {
    assert_eq!(args("$plugins"), Some(vec!["git".into(), "fzf".into()]));
    assert_eq!(
        args(r#""${plugins[@]}""#),
        Some(vec!["git".into(), "fzf".into()])
    );
    assert_eq!(scalar(r#""$plugins""#, &env()), Some("git fzf".into()));
}

#[test]
fn unquoted_empty_words_disappear() {
    assert_eq!(args("$GONE"), Some(Vec::new()));
    assert_eq!(args(r#""$GONE""#), Some(vec![String::new()]));
}

#[test]
fn braces_expand_before_globbing() {
    let root = tempfile::tempdir().unwrap();
    for name in ["01-a.zsh", "02-b.zsh", "10-c.zsh", "x.txt"] {
        std::fs::write(root.path().join(name), "").unwrap();
    }
    let base = root.path().display();
    let found = args(&format!("{base}/{{0[2-9],[1-9][0-9]}}-*.zsh(N)")).unwrap();
    assert_eq!(
        found,
        vec![format!("{base}/02-b.zsh"), format!("{base}/10-c.zsh")]
    );
    assert_eq!(args(&format!("{base}/*.none(N)")), Some(Vec::new()));
    assert_eq!(args(&format!("{base}/*.none")), None);
    assert_eq!(
        args("{1..3}"),
        Some(vec!["1".into(), "2".into(), "3".into()])
    );
}

#[test]
fn qualifiers_other_than_null_glob_are_not_modelled() {
    assert_eq!(args("/tmp/*(N-/)"), None);
}

#[test]
fn patterns_match_quoted_parts_literally() {
    let pattern = pattern(r#"*"$ZSH/*"*"#, &env()).unwrap();
    assert!(pattern.matches("x:/home/me/.oh-my-zsh/*:y"));
    assert!(!pattern.matches("x:/home/me/.oh-my-zsh/lib:y"));
}
