use std::path::PathBuf;

use super::*;
use crate::script::parse;

fn hazard_of(text: &str) -> Option<&'static str> {
    hazard(&parse(text).unwrap().lists)
}

fn compinit_in(text: &str) -> bool {
    calls_compinit(&parse(text).unwrap().lists)
}

#[test]
fn top_level_code_that_changes_meaning_in_a_function_is_a_hazard() {
    assert_eq!(hazard_of("typeset x=1"), Some("declaration without -g"));
    assert_eq!(hazard_of("integer count"), Some("declaration without -g"));
    assert_eq!(hazard_of("local x"), Some("local outside a function"));
    assert_eq!(hazard_of("trap 'rm -f x' EXIT"), Some("trap"));
    assert_eq!(hazard_of("emulate -L zsh"), Some("local options"));
    assert_eq!(hazard_of("setopt LOCAL_OPTIONS"), Some("local options"));
    assert_eq!(hazard_of("[[ -n $x ]] || return"), Some("return"));
    assert_eq!(hazard_of("break"), Some("loop control outside a loop"));
}

#[test]
fn global_and_function_scoped_code_is_not_a_hazard() {
    for text in [
        "typeset -g x=1",
        "typeset -gA table",
        "export X=1",
        "typeset -f name",
        "typeset",
        "setopt extended_glob",
        "f() { local x; typeset y; return 1 }",
        "() { local x; trap 'print' EXIT }",
        "for x in a b; do break; done",
        "repeat 1 do\n  [[ -n $x ]] && break\ndone",
        "alias ll='ls -l'",
    ] {
        assert_eq!(hazard_of(text), None, "{text}");
    }
}

#[test]
fn compinit_counts_when_it_runs_now() {
    assert!(compinit_in("autoload -Uz compinit\ncompinit -u -d x"));
    assert!(compinit_in("if [[ -n $x ]]; then\n  compinit -i\nfi"));
    assert!(compinit_in("() {\n  compinit -C\n} plugins/x"));
    assert!(!compinit_in("f() { compinit }"));
    assert!(!compinit_in("autoload -U compaudit compinit zrecompile"));
}

#[test]
fn the_compinit_window_spans_from_the_dump_path_to_omz_source() {
    let text = "autoload -U compinit\n\
                if [[ -z $ZSH_COMPDUMP ]]; then\n  ZSH_COMPDUMP=$HOME/.zcompdump\nfi\n\
                zcompdump_refresh=0\n\
                compinit -u -d \"$ZSH_COMPDUMP\"\n\
                _omz_source() { source \"$ZSH/$1\" }\n\
                _omz_source lib/x.zsh\n";
    assert_eq!(window_lists(&parse(text).unwrap().lists), Some(2..4));
    assert_eq!(
        window_lists(&parse("compinit\n_omz_source() { : }\n").unwrap().lists),
        None
    );
}

#[test]
fn selection_matches_plugin_names_eval_names_and_root_relative_files() {
    let root = PathBuf::from("/dots");
    let deferral = Deferral::new(
        &config::Defer {
            compinit: true,
            plugins: vec!["git*".into(), "npm".into()],
            evals: vec!["*-completion".into()],
            files: vec!["zsh/7*-completions.zsh".into()],
        },
        &root,
    )
    .unwrap();
    assert!(deferral.plugin("plugins/gitignore/gitignore.plugin.zsh"));
    assert!(deferral.plugin("plugins/npm/npm.plugin.zsh"));
    assert!(!deferral.plugin("plugins/fzf/fzf.plugin.zsh"));
    assert!(!deferral.plugin("lib/git.zsh"));
    assert!(deferral.eval("tool-completion"));
    assert!(!deferral.eval("starship-init"));
    assert!(deferral.file(&root.join("zsh/70-completions.zsh")));
    assert!(!deferral.file(Path::new("/elsewhere/zsh/70-completions.zsh")));
}
