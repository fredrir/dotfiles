use super::*;

fn kinds(text: &str) -> Vec<Kind> {
    lex(text).into_iter().map(|token| token.kind).collect()
}

#[test]
fn paths_variables_formats_and_module_calls_are_single_tokens() {
    assert_eq!(
        kinds(".a.b[0] | $x::y | @csv | m::f(..)"),
        [
            Kind::Field("a".into()),
            Kind::Field("b".into()),
            Kind::Punct("["),
            Kind::Num,
            Kind::Punct("]"),
            Kind::Punct("|"),
            Kind::Var("x::y".into()),
            Kind::Punct("|"),
            Kind::Format("csv".into()),
            Kind::Punct("|"),
            Kind::Ident("m::f".into()),
            Kind::Punct("("),
            Kind::Recurse,
            Kind::Punct(")"),
        ]
    );
}

#[test]
fn partial_words_at_the_end_are_still_tokens() {
    assert_eq!(kinds("$"), [Kind::Var(String::new())]);
    assert_eq!(kinds("@"), [Kind::Format(String::new())]);
    assert_eq!(kinds("m::"), [Kind::Ident("m::".into())]);
    assert_eq!(kinds(". .5"), [Kind::Dot, Kind::Num]);
}

#[test]
fn interpolations_split_their_string() {
    assert_eq!(
        kinds(r#""a \(.b + "\(.c)") d" | ."e""#),
        [
            Kind::Str(Part::Start),
            Kind::Field("b".into()),
            Kind::Punct("+"),
            Kind::Str(Part::Start),
            Kind::Field("c".into()),
            Kind::Str(Part::End),
            Kind::Str(Part::End),
            Kind::Punct("|"),
            Kind::Dot,
            Kind::Str(Part::Whole),
        ]
    );
}

#[test]
fn an_unterminated_string_is_open() {
    let tokens = lex(r#"."my \" ke"#);
    assert_eq!(tokens.len(), 2);
    assert!(tokens[1].open);
    assert!(!lex(r#""done""#)[0].open);
}

#[test]
fn comments_run_to_the_end_of_the_line() {
    assert_eq!(
        kinds(".a # .b | .c\n| .d"),
        [
            Kind::Field("a".into()),
            Kind::Comment,
            Kind::Punct("|"),
            Kind::Field("d".into()),
        ]
    );
}

#[test]
fn keywords_do_not_end_terms() {
    let tokens = lex("then end and f");
    assert!(!tokens[0].ends_term());
    assert!(!tokens[2].ends_term());
    assert!(tokens[3].ends_term());
}
