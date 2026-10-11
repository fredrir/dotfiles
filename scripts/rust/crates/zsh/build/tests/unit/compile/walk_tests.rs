use super::*;
use crate::script::parse;

fn sites(text: &str) -> Option<Vec<(Option<usize>, Option<String>)>> {
    returns(&parse(text).unwrap().lists).map(|found| {
        found
            .into_iter()
            .map(|site| (site.depth, site.argument))
            .collect()
    })
}

#[test]
fn returns_are_tagged_by_scope_and_loop_depth() {
    let text = "[[ -o i ]] || return 0\nf() { return 1 }\nfor x in a; do\n  return\ndone\n";
    assert_eq!(
        sites(text),
        Some(vec![
            (Some(0), Some("0".into())),
            (None, Some("1".into())),
            (Some(1), None)
        ])
    );
}

#[test]
fn a_return_in_a_subshell_cannot_be_rewritten() {
    assert_eq!(sites("( return 1 )"), None);
    assert_eq!(
        file_returns(&parse("f() { ( return 1 ) }").unwrap().lists),
        Some(0)
    );
}

#[test]
fn fingerprints_ignore_layout_and_comments() {
    let a = parse("f() {\n  # note\n  print x\n}").unwrap();
    let b = parse("f() { print x }").unwrap();
    let c = parse("f() { print y }").unwrap();
    let fingerprint_of = |program: &zshrs_parse::parser::ZshProgram| match sole(&program.lists[0]) {
        Some(ZshCommand::FuncDef(node)) => fingerprint(node),
        _ => panic!("function expected"),
    };
    assert_eq!(fingerprint_of(&a), fingerprint_of(&b));
    assert_ne!(fingerprint_of(&b), fingerprint_of(&c));
}

#[test]
fn assigned_names_cover_declarations_and_loops() {
    let names = assigned(
        &parse("a=1 b+=(x)\nexport C=1 -x\nfor d in x; do typeset -g e; done")
            .unwrap()
            .lists,
    );
    for name in ["a", "b", "C", "d", "e"] {
        assert!(names.contains(&name.to_string()), "{name} in {names:?}");
    }
}

#[test]
fn defined_names_skip_nested_and_anonymous_functions() {
    let program = parse("a() { b() { : } }\nif true; then function c { : }; fi\n() { : }").unwrap();
    assert_eq!(
        function_names(&program.lists),
        vec!["a".to_string(), "c".to_string()]
    );
}

#[test]
fn declared_names_are_found_in_unparsed_code() {
    let text = "  fzf-file-widget() {\nfunction __fzf_select {\nx=1 # () {\nfoo bar() {\n";
    assert_eq!(
        declared_names(text),
        vec!["fzf-file-widget".to_string(), "__fzf_select".to_string()]
    );
}

#[test]
fn command_tests_name_one_plain_command() {
    assert_eq!(
        command_test(" $+commands[ack-grep] "),
        Some(("ack-grep", false))
    );
    assert_eq!(command_test("${+commands[less]}"), Some(("less", false)));
    assert_eq!(command_test(" ! $+commands[gh] "), Some(("gh", true)));
    assert_eq!(command_test("!${+commands[fzf]}"), Some(("fzf", true)));
    for other in [
        "$+commands[$1]",
        "$+commands[a/b]",
        "$+commands[@]",
        "$+commands[x] && $+commands[y]",
        "${+commands[x]",
        "$+functions[x]",
        "! ! $+commands[x]",
    ] {
        assert_eq!(command_test(other), None, "{other}");
    }
}

#[test]
fn command_tests_read_the_parsed_arithmetic() {
    let program = parse("(( ! ${+commands[fzf]} ))").unwrap();
    let Some(ZshCommand::Arith(expr)) = sole(&program.lists[0]) else {
        panic!("arithmetic expected");
    };
    assert_eq!(command_test(&text(expr)), Some(("fzf", true)));
}

#[test]
fn only_a_lasting_emulation_other_than_zsh_changes_it() {
    let changes =
        |text: &str, in_function| changes_emulation_in(&parse(text).unwrap().lists, in_function);
    for text in [
        "emulate",
        "emulate zsh",
        "emulate -R zsh -o kshglob",
        "emulate sh -c true",
        "f() { emulate -L sh }",
        "( emulate sh )",
    ] {
        assert!(!changes(text, false), "{text}");
    }
    for text in [
        "emulate sh",
        "emulate -L sh",
        "emulate -o kshglob sh",
        "'builtin' 'emulate' 'ksh'",
        "f() { emulate sh }",
        "eval 'emulate sh'",
        "emulate $mode",
    ] {
        assert!(changes(text, false), "{text}");
    }
    assert!(!changes("emulate -L sh", true));
}

#[test]
fn option_changes_are_found_outside_nested_functions() {
    let sets = |text: &str| sets_options(&parse(text).unwrap().lists);
    for text in [
        "setopt shglob",
        "if x; then unsetopt aliases; fi",
        "options[aliases]=off",
        "builtin emulate zsh",
    ] {
        assert!(sets(text), "{text}");
    }
    assert!(!sets("f() { setopt shglob }"));
    assert!(!sets("print setopt"));
}

#[test]
fn a_status_is_read_before_any_function_body() {
    let first = |text: &str| parse(text).unwrap().lists.remove(0);
    for text in [
        "print $?",
        "x=${?}",
        "return",
        "builtin exit",
        "print $status",
        "(( pipestatus[1] ))",
    ] {
        assert!(reads_status(&first(text)), "{text}");
    }
    for text in ["return 1", "f() { local s=$? }", "print ok"] {
        assert!(!reads_status(&first(text)), "{text}");
    }
}

#[test]
fn top_level_text_leaves_out_function_bodies() {
    let lists = parse("print $funcstack\nf() { print $LINENO }")
        .unwrap()
        .lists;
    let text = top_level_text(&lists);
    assert!(text.contains("$funcstack"), "{text}");
    assert!(text.contains('f'), "{text}");
    assert!(!text.contains("LINENO"), "{text}");
}

#[test]
fn replacements_reach_only_function_bodies() {
    let lists = parse("eval x\nf() { eval x; if true; then eval x; fi }")
        .unwrap()
        .lists;
    let block = || ZshCommand::Cursh(Box::new(parse("y").unwrap()));
    let replaced = replace_in_functions(&lists, &|simple| {
        (words(simple) == ["eval", "x"]).then(block)
    })
    .unwrap();
    let expected = parse("eval x\nf() { { y }; if true; then { y }; fi }")
        .unwrap()
        .lists;
    assert_eq!(replaced.len(), expected.len());
    for (left, right) in replaced.iter().zip(&expected) {
        assert_eq!(crate::script::shape(left), crate::script::shape(right));
    }
}

fn single(text: &str) -> Vec<(usize, usize)> {
    single_calls(&parse(text).unwrap().lists, text)
}

#[test]
fn a_function_called_once_then_removed_runs_only_there() {
    let text = "use_a() { a }\nuse_b() { b }\nuse_a || use_b\nunset -f -m 'use_*'\nafter\n";
    assert_eq!(single(text), [(0, 2)]);
    assert_eq!(single("f() { a }\nf\nunfunction f\n"), [(0, 1)]);
}

#[test]
fn a_function_that_may_run_again_is_not_run_once() {
    for text in [
        "f() { a }\nf\n",
        "f() { a }\nx=1\nf\nunset -f f\n",
        "f() { a }\nf\ng\nunset -f f\n",
        "f() { a }\nf || return\nunset -f f\n",
        "f() { a }\nf\nunset f\n",
        "f() { f }\nf\nunset -f f\n",
        "f() { a }\nf\nunset -f -m 'g*'\n",
        "f() { a }\n# f\nf\nunset -f f\n",
    ] {
        assert!(single(text).is_empty(), "{text}");
    }
}
