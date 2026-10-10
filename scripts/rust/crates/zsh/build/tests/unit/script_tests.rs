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
    assert!(script.joined);
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
    assert!(!Script::parse("a=(\n  b\n)\n".to_string()).unwrap().joined);
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
