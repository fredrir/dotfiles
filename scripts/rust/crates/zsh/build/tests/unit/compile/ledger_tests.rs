use super::*;
use crate::script::parse;

fn touches(name: &str, values: &[&str]) -> Vec<(Touch, Key)> {
    let values: Vec<String> = values.iter().map(|value| value.to_string()).collect();
    command_touches(name, &values)
}

fn define(kind: &'static str, name: &str) -> (Touch, Key) {
    (Touch::Define, (kind, name.to_string()))
}

fn body(text: &str) -> Names {
    body_names(&parse(text).unwrap().lists)
}

#[test]
fn commands_define_names_in_the_shell_tables() {
    assert_eq!(
        touches("alias", &["-g", "ll=ls -l", "la"]),
        vec![define("alias", "ll")]
    );
    assert_eq!(
        touches("unalias", &["ga", "gc"]),
        vec![define("alias", "ga"), define("alias", "gc")]
    );
    assert_eq!(
        touches("unset", &["-f", "f"]),
        vec![define("function", "f")]
    );
    assert_eq!(
        touches("setopt", &["NO_AUTO_CD"]),
        vec![define("option", "autocd")]
    );
    assert_eq!(
        touches("zstyle", &[":completion:*", "menu", "select"]),
        vec![define("zstyle", ":completion:* menu")]
    );
    assert!(touches("zstyle", &["-s", ":x", "style", "var"]).is_empty());
    assert_eq!(
        touches("zle", &["-N", "widget", "fn"]),
        vec![define("widget", "widget")]
    );
    assert_eq!(
        touches("compdef", &["_git", "gg", "ggpnp=git-checkout"]),
        vec![define("completion", "gg"), define("completion", "ggpnp")]
    );
    assert_eq!(
        touches("compdef", &["-d", "x"]),
        vec![define("completion", "x")]
    );
    assert_eq!(
        touches("add-zsh-hook", &["chpwd", "hook"]),
        vec![define("var", "chpwd_functions")]
    );
    assert_eq!(
        touches("git_main_branch", &[]),
        vec![(Touch::Call, ("function", "git_main_branch".to_string()))]
    );
}

#[test]
fn names_known_only_at_runtime_stand_for_any_name() {
    assert_eq!(
        touches("alias", &["\"$1\"=eza -l"]),
        vec![define("alias", "*")]
    );
    assert_eq!(
        touches("unfunction", &["$fn"]),
        vec![define("function", "*")]
    );
    assert!(touches("$cmd", &["x"]).is_empty());
}

#[test]
fn bodies_skip_their_locals_and_local_options() {
    let names = body(
        "local count\ncount=1\nshared=1\nsetopt localoptions extendedglob\nalias \"$1\"=x\nhelper\nIFS= read -r line",
    );
    assert!(names.defined.contains(&("var", "shared".into())));
    assert!(!names.defined.contains(&("var", "count".into())));
    assert!(!names.defined.contains(&("var", "IFS".into())));
    assert!(!names.defined.iter().any(|key| key.0 == "option"));
    assert!(names.defined.contains(&("alias", "*".into())));
    assert!(names.called.contains(&("function", "helper".into())));
}

#[test]
fn later_definitions_and_calls_conflict_with_what_deferred_code_defines() {
    let mut names = Names::default();
    names.defined.insert(("alias", "ll".into()));
    names.defined.insert(("function", "helper".into()));
    names.called.insert(("function", "compdef".into()));
    let key = |kind: &'static str, name: &str| (kind, name.to_string());
    assert!(names.conflict(Touch::Define, &key("alias", "ll")).is_some());
    assert!(
        names
            .conflict(Touch::Call, &key("function", "helper"))
            .is_some()
    );
    assert!(
        names
            .conflict(Touch::Call, &key("function", "compdef"))
            .is_none()
    );
    assert!(
        names
            .conflict(Touch::Define, &key("function", "compdef"))
            .is_some()
    );
    assert!(names.conflict(Touch::Define, &key("alias", "la")).is_none());
    names.defined.insert(("alias", "*".into()));
    assert_eq!(
        names
            .conflict(Touch::Define, &key("alias", "la"))
            .as_deref(),
        Some("alias names set at runtime, alias la changed later")
    );
}

#[test]
fn rolling_back_drops_what_dead_code_recorded() {
    let mut ledger = Ledger::default();
    ledger.event(Touch::Define, ("alias", "kept".into()));
    let mark = ledger.mark();
    ledger.event(Touch::Define, ("alias", "dead".into()));
    ledger.compinit = Some(0);
    ledger.reset(mark);
    assert_eq!(
        ledger.events,
        vec![(Touch::Define, ("alias", "kept".into()))]
    );
    assert_eq!(ledger.compinit, None);
}

#[test]
fn compdef_after_a_deferred_compinit_is_queued_behind_it() {
    let mut ledger = Ledger {
        compinit: Some(0),
        ..Ledger::default()
    };
    ledger.event(Touch::Define, ("completion", "gg".into()));
    ledger.event(Touch::Define, ("alias", "gg".into()));
    assert_eq!(ledger.events[0].0, Touch::Queued(0));
    assert_eq!(ledger.events[1].0, Touch::Define);
}
