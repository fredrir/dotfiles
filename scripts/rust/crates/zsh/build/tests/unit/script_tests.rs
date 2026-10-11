use super::*;

fn owned(text: &str) -> Vec<Option<(usize, usize)>> {
    let script = Script::parse(text.to_string()).unwrap();
    script
        .segments(&script.program.lists, script.line_count())
        .into_iter()
        .map(|segment| segment.map(|segment| (*segment.lines.start(), *segment.lines.end())))
        .collect()
}

#[test]
fn statements_own_their_lines_and_trailing_comments() {
    let text = "a=1\n# note\nif true; then\n  b\nfi\n\nc <<EOF\nbody\nEOF\nd\n";
    assert_eq!(
        owned(text),
        vec![Some((1, 2)), Some((3, 6)), Some((7, 9)), Some((10, 10))]
    );
}

#[test]
fn statements_on_one_line_share_a_segment() {
    assert_eq!(owned("a; b\nc\n"), vec![Some((1, 1)), Some((2, 2))]);
}

#[test]
fn anonymous_functions_reproduce_despite_counter_names() {
    assert_eq!(
        owned("() {\n  local x\n}\n() {\n  :\n}\n"),
        vec![Some((1, 3)), Some((4, 6))]
    );
}

#[test]
fn simple_commands_reprint_to_the_same_command() {
    let program = parse(r#"x=1 source "$ZCONF/a b.zsh" 'q'"#).unwrap();
    let zshrs_parse::parser::ZshCommand::Simple(simple) = &program.lists[0].sublist.pipe.cmd else {
        panic!("simple command expected");
    };
    assert_eq!(
        reprint(simple).unwrap(),
        r#"x=1 source "$ZCONF/a b.zsh" 'q'"#
    );
}

#[test]
fn array_lines_starting_like_commands_stay_elements() {
    let text = "f() {\n  a=( do\n    if { [[\n    FOO=1 (x|y)\n  )\n}\nb=(\n  c\n)\n";
    let script = Script::parse(text.to_string()).unwrap();
    let ZshCommand::FuncDef(function) = &script.program.lists[0].sublist.pipe.cmd else {
        panic!("function expected");
    };
    let one_line = parse("a=( do if { [[ FOO=1 (x|y) )").unwrap();
    assert!(same_lists(&function.body.lists, &one_line.lists));
    assert!(script.reading.joined);
    assert_eq!(start_line(&script.program.lists[1]), 7);
    assert_eq!(
        script
            .segments(&script.program.lists, script.line_count())
            .into_iter()
            .map(|segment| segment.map(|segment| *segment.lines.end()))
            .collect::<Vec<_>>(),
        vec![Some(6), Some(9)]
    );
}

#[test]
fn arrays_that_parse_as_written_are_not_joined() {
    assert!(
        !Script::parse("a=(\n  b\n)\n".to_string())
            .unwrap()
            .reading
            .joined
    );
    assert!(parse("a=( b # note\n  if )").is_err());
}

#[test]
fn incomplete_input_is_an_error() {
    assert!(parse("if true; then").is_err());
    assert!(parse("f() {\n  :").is_err());
    assert!(parse("cat <<EOF\nbody").is_err());
    assert!(parse("a &&").is_err());
    assert!(parse("echo $(").is_err());
    assert!(parse("a\n# trailing").is_ok());
}

#[test]
fn subshells_around_double_parens_are_respelled_with_a_space() {
    let text = "if ((( a )) && [[ $s == x ]]); then :; fi\n";
    assert!(parse(text).is_err());
    let respelled = respace(text).unwrap();
    assert_eq!(respelled, "if ( (( a )) && [[ $s == x ]]); then :; fi\n");
    assert!(parse(&respelled).is_ok());
    assert_eq!(respace("(((a + 1) * 2)) && :\n"), None);
}

/// The else body of the `if` that `text` starts with.
fn else_body(text: &str) -> Vec<ZshList> {
    let program = parse(text).unwrap();
    let ZshCommand::If(node) = &program.lists[0].sublist.pipe.cmd else {
        panic!("if expected");
    };
    node.else_.as_ref().unwrap().lists.clone()
}

#[test]
fn a_block_starting_an_else_body_stays_in_it() {
    let block = else_body("if c; then\n  :\nelse\n  { a; }\nfi\n");
    assert!(
        matches!(block.as_slice(), [list] if matches!(list.sublist.pipe.cmd, ZshCommand::Cursh(_)))
    );
    let tried = else_body("if c; then\n  :\nelse\n{\na\n} always { TRY_BLOCK_ERROR=0 }\nfi\n");
    assert!(
        matches!(tried.as_slice(), [list] if matches!(list.sublist.pipe.cmd, ZshCommand::Try(_)))
    );
    let program = parse("f() {\n  if c; then :; else\n    { a; }\n  fi\n}\n").unwrap();
    assert!(!serde_json::to_string(&program).unwrap().contains(ELSE_MARK));
}

#[test]
fn a_brace_form_else_reads_as_before() {
    let body = else_body("if [[ -n $c ]] {\n  a\n} else {\n  b\n}\n");
    assert!(
        matches!(body.as_slice(), [list] if matches!(list.sublist.pipe.cmd, ZshCommand::Simple(_)))
    );
}

#[test]
fn an_else_argument_is_not_marked() {
    let program = parse("print else\n{ a; }\nif c; then\n  :\nelse\n  { b; }\nfi\n").unwrap();
    let ZshCommand::Simple(simple) = &program.lists[0].sublist.pipe.cmd else {
        panic!("simple command expected");
    };
    assert_eq!(simple.words.len(), 2);
    assert!(matches!(
        program.lists[1].sublist.pipe.cmd,
        ZshCommand::Cursh(_)
    ));
}

#[test]
fn statements_around_an_else_block_keep_their_lines() {
    let text = "if c; then\n  :\nelse\n  { a; }\nfi\nafter\n";
    let program = parse(text).unwrap();
    assert_eq!(start_line(&program.lists[1]), 6);
    assert_eq!(start_line(&else_body(text)[0]), 4);
    assert_eq!(owned(text), vec![Some((1, 5)), Some((6, 6))]);
}

/// The kind of each statement in the then and else bodies of the `if` in `list`.
fn arms(list: &ZshList) -> (Vec<&'static str>, Vec<&'static str>) {
    let ZshCommand::If(node) = &list.sublist.pipe.cmd else {
        panic!("if expected");
    };
    let kinds = |program: &ZshProgram| {
        program
            .lists
            .iter()
            .map(|list| match list.sublist.pipe.cmd {
                ZshCommand::Simple(_) => "simple",
                ZshCommand::Cursh(_) => "block",
                ZshCommand::Try(_) => "try",
                _ => "other",
            })
            .collect()
    };
    (kinds(&node.then), node.else_.as_deref().map(kinds).unwrap())
}

#[test]
fn brace_and_then_forms_in_one_file_each_keep_their_else_body() {
    let text =
        "if [[ -n $c ]] {\n  a\n} else {\n  b\n}\nif d; then\n  :\nelse\n  { e; }\nfi\nafter\n";
    let program = parse(text).unwrap();
    assert_eq!(arms(&program.lists[0]), (vec!["simple"], vec!["simple"]));
    assert_eq!(arms(&program.lists[1]), (vec!["simple"], vec!["block"]));
    assert_eq!(start_line(&program.lists[2]), 11);
    assert_eq!(
        owned(text),
        vec![Some((1, 5)), Some((6, 10)), Some((11, 11))]
    );
}

#[test]
fn a_then_body_ending_in_a_block_keeps_the_else_block() {
    for (text, then) in [
        ("if c; then\n  { a; }\nelse\n  { b; }\nfi\n", "block"),
        (
            "if c; then\n{\na\n} always { TRY_BLOCK_ERROR=0 }\nelse\n{ b; }\nfi\n",
            "try",
        ),
        ("if c; then { a; } else\n{ b; }\nfi\n", "block"),
    ] {
        let program = parse(text).unwrap();
        assert_eq!(
            arms(&program.lists[0]),
            (vec![then], vec!["block"]),
            "{text}"
        );
    }
}

#[test]
fn an_else_after_a_brace_is_marked_as_its_if_reads() {
    let text = "if [[ -n $c ]] { a; } else { b; }\nif d; then { e; } else\n{ f; }\nfi\nafter\n";
    let program = parse(text).unwrap();
    assert_eq!(arms(&program.lists[0]), (vec!["simple"], vec!["simple"]));
    assert_eq!(arms(&program.lists[1]), (vec!["block"], vec!["block"]));
    assert_eq!(start_line(&program.lists[2]), 5);
    assert!(!serde_json::to_string(&program).unwrap().contains(ELSE_MARK));
}

#[test]
fn ambiguous_marks_are_tried_fewest_first_up_to_a_cap() {
    assert_eq!(mark_choices(0), vec![Vec::<bool>::new()]);
    assert_eq!(
        mark_choices(2),
        vec![
            vec![false, false],
            vec![true, false],
            vec![false, true],
            vec![true, true]
        ]
    );
    assert_eq!(mark_choices(MAX_UNSURE).len(), 1 << MAX_UNSURE);
    let past = mark_choices(MAX_UNSURE + 1);
    assert_eq!(past.len(), 2);
    assert!(past[0].iter().all(|marked| !marked) && past[1].iter().all(|marked| *marked));
}

#[test]
fn an_else_takes_then_from_its_own_if() {
    let reserved = |text: &'static str| -> Vec<(&'static str, usize)> {
        text.split_whitespace().map(|word| (word, 0)).collect()
    };
    assert!(takes_then(&reserved("if then")));
    assert!(takes_then(&reserved("if then if then fi")));
    assert!(!takes_then(&reserved("if")));
    assert!(!takes_then(&reserved("if then if")));
    assert!(takes_then(&reserved("if then elif then")));
    let text = "if a; then\n  if b; then c; fi\n  { d; } else\n{ e; }\nfi\n";
    let program = parse(text).unwrap();
    assert_eq!(
        arms(&program.lists[0]),
        (vec!["other", "block"], vec!["block"])
    );
}
