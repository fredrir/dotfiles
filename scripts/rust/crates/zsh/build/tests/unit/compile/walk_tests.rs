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
