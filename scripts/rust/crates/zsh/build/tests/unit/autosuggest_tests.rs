use super::*;

fn dump(records: &[(&str, &str, &str)]) -> String {
    records
        .iter()
        .chain(&[("end", "", "")])
        .map(|(table, name, value)| format!("{table}\0{name}\0{value}\0"))
        .collect()
}

fn recorded() -> Binding {
    Binding::parse(&dump(&[
        ("key", "", "beep=builtin\nyank=builtin"),
        ("functions", "_zsh_autosuggest_orig_yank", "\tzle .yank"),
        (
            "functions",
            "_zsh_autosuggest_bound_1_yank",
            "\t_zsh_autosuggest_widget_modify autosuggest-orig-1-yank $@",
        ),
        ("widgets", "autosuggest-orig-1-yank", "user:_zsh_autosuggest_orig_yank"),
        ("widgets", "yank", "user:_zsh_autosuggest_bound_1_yank"),
        (
            "widgets",
            "autosuggest-orig-1-complete-word",
            "completion:.complete-word:_main_complete",
        ),
        ("parameters", COUNTS, "association"),
        ("counts", "yank", "1"),
    ]))
    .unwrap()
}

#[test]
fn a_recorded_binding_becomes_definitions_and_zle_calls() {
    let code = recorded().code().unwrap();
    assert!(code.contains("[[ $REPLY == $'beep=builtin\\nyank=builtin' ]] || return 1\n"));
    assert!(code.contains("_zsh_autosuggest_orig_yank() {\n\tzle .yank\n}\n"));
    assert!(code.contains("zle -N -- yank _zsh_autosuggest_bound_1_yank\n"));
    assert!(code.contains(
        "zle -C -- autosuggest-orig-1-complete-word .complete-word _main_complete\n"
    ));
    assert!(code.contains("typeset -gA _ZSH_AUTOSUGGEST_BIND_COUNTS=(yank 1)\n"));
    assert!(code.contains("builtin functions -c _zsh_autosuggest_bind_widgets _zsh_build_bind_widgets\n"));
}

#[test]
fn a_binding_with_other_changes_is_left_to_the_plugin() {
    for (records, reason) in [
        (vec![("parameters", "OTHER", "scalar")], "binding sets OTHER"),
        (
            vec![("removed", "widget", "yank")],
            "binding removes widget yank",
        ),
        (vec![("unbound", "", "")], "binding did not run"),
    ] {
        assert_eq!(Binding::parse(&dump(&records)), Err(reason.to_string()));
    }
    assert!(Binding::parse("key\0\0").is_err());
}

#[test]
fn names_or_widgets_the_code_cannot_spell_are_left_to_the_plugin() {
    let mut binding = recorded();
    binding.functions.push(("odd name".to_string(), ":".to_string()));
    assert!(binding.code().is_err());
    let mut binding = recorded();
    binding.widgets.push(("w".to_string(), "builtin".to_string()));
    assert!(binding.code().is_err());
}
