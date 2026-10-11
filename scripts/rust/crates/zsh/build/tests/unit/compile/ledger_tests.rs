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
    read_body(&parse(text).unwrap().lists, true).names
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

#[test]
fn builtin_runs_the_command_it_names() {
    assert_eq!(
        touches("builtin", &["setopt", "no_aliases"]),
        vec![define("option", "aliases")]
    );
}

#[test]
fn bindkey_in_a_substitution_reads_the_binding() {
    let names = body("binding=$(builtin bindkey '^I')\nbindkey '^T' widget\n");
    assert!(names.called.contains(&("bindkey", "main ^I".to_string())));
    assert!(names.defined.contains(&("bindkey", "main ^T".to_string())));
}

#[test]
fn a_runtime_key_binding_conflicts_with_any_key_of_its_keymap() {
    let names = body("bindkey -M vicmd \"$1\" widget\n");
    let later = |name: &str| ("bindkey", name.to_string());
    assert!(names.conflict(Touch::Define, &later("vicmd ^R")).is_some());
    assert!(names.conflict(Touch::Define, &later("main ^R")).is_none());
}

#[test]
fn what_a_callee_sets_in_its_callers_locals_stays_local() {
    let mut ledger = Ledger::default();
    for (name, text) in [
        ("helper", "REPLY=1\nalias h=x"),
        ("scoped", "local REPLY\nhelper"),
        ("both", "scoped\nhelper"),
    ] {
        let body = read_body(&parse(text).unwrap().lists, true);
        ledger.bodies.insert(name.to_string(), body);
    }
    let run = |text: &str| ledger.running(&read_body(&parse(text).unwrap().lists, true));
    let reply = define("var", "REPLY");
    assert!(!run("scoped").contains(&reply));
    assert!(run("scoped").contains(&define("alias", "h")));
    assert!(run("both").contains(&reply));
    assert!(!run("() { local REPLY; helper }").contains(&reply));
    assert!(run("() { local REPLY }\nhelper").contains(&reply));
}

#[test]
fn declarations_set_what_they_assign_unless_it_is_local() {
    let names = body(
        "typeset -g kept=1\nexport EXPORTED=1\nlocal own=1\ntypeset scoped=1\nexport \"$1=$2\"",
    );
    assert!(names.defined.contains(&("var", "kept".into())));
    assert!(names.defined.contains(&("var", "EXPORTED".into())));
    assert!(names.defined.contains(&("var", "*".into())));
    assert!(!names.defined.contains(&("var", "own".into())));
    assert!(!names.defined.contains(&("var", "scoped".into())));
    let top = read_body(
        &parse("typeset scoped=1\nlocal own\nown=1\nsetopt localoptions autocd")
            .unwrap()
            .lists,
        false,
    );
    assert!(top.names.defined.contains(&("var", "scoped".into())));
    assert!(top.names.defined.contains(&("var", "own".into())));
    assert!(top.names.defined.contains(&("option", "autocd".into())));
}
