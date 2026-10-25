use super::*;

fn word(text: &str) -> Token {
    Token::Word(text.into())
}

fn file(path: &str) -> Input {
    Input::File(path.into())
}

#[test]
fn words_lose_their_quotes_and_operators_stand_apart() {
    assert_eq!(
        tokens(r#"cat 'a b.json' "c\"d" $'e\tf'|jq -r"#),
        [
            word("cat"),
            word("a b.json"),
            word("c\"d"),
            word("e\tf"),
            Token::Op("|"),
            word("jq"),
            word("-r"),
        ]
    );
}

#[test]
fn a_descriptor_number_belongs_to_its_redirection() {
    assert_eq!(
        tokens("cat a.json 2>/dev/null | jq"),
        [
            word("cat"),
            word("a.json"),
            Token::Op(">"),
            word("/dev/null"),
            Token::Op("|"),
            word("jq"),
        ]
    );
}

#[test]
fn an_unterminated_quote_ends_with_the_word() {
    assert_eq!(unquote("'.foo.ba"), ".foo.ba");
    assert_eq!(unquote(r#""\$x | .y"#), "$x | .y");
    assert_eq!(unquote(r"\$x"), "$x");
    assert_eq!(unquote("''"), "");
}

#[test]
fn files_piped_through_cat_feed_the_command() {
    assert_eq!(
        stdin("x=1; cat -- a.json 'b c.json' | jq -r ", ""),
        [file("a.json"), file("b c.json")]
    );
    assert_eq!(stdin("command cat < a.json | jq ", ""), [file("a.json")]);
}

#[test]
fn redirections_before_and_after_the_word_feed_the_command() {
    assert_eq!(
        stdin("< a.json jq ", " < b.json | less"),
        [file("a.json"), file("b.json")]
    );
    assert_eq!(stdin("jq ", " | jq . < c.json"), []);
}

#[test]
fn echoed_text_and_here_strings_feed_the_command_as_text() {
    assert_eq!(
        stdin(r#"echo -n '{"a":1}' | jq "#, " <<< '[1]'"),
        [
            Input::Text("{\"a\":1}\n".into()),
            Input::Text("[1]\n".into())
        ]
    );
}

#[test]
fn producers_that_would_have_to_run_are_ignored() {
    assert_eq!(stdin("curl -s https://example.com | jq ", ""), []);
    assert_eq!(stdin("echo $(cat a.json) > out; jq ", ""), []);
    assert_eq!(stdin("cat a.json; jq ", ""), []);
}
