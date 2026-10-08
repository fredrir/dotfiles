use super::*;

fn substitutions(text: &str) -> Vec<(&str, bool, bool)> {
    scan(text)
        .substitutions
        .iter()
        .map(|found| (&text[found.inner.clone()], found.quoted, found.in_param))
        .collect()
}

#[test]
fn substitutions_record_their_quoting() {
    assert_eq!(
        substitutions(r#"a=$(brew --prefix)/x; eval "$(fzf --zsh)""#),
        vec![("brew --prefix", false, false), ("fzf --zsh", true, false)]
    );
}

#[test]
fn substitutions_nested_in_parameters_are_marked() {
    assert_eq!(
        substitutions(r#"v="${${(As: :)$(git version)}[3]}""#),
        vec![("git version", true, true)]
    );
}

#[test]
fn backticks_are_substitutions() {
    assert_eq!(
        substitutions("eval `path_helper -s`"),
        vec![("path_helper -s", false, false)]
    );
}

#[test]
fn single_quotes_and_comments_hide_expansions() {
    assert!(substitutions("echo '$(no)' # $(no)\n").is_empty());
    assert!(scan("print '$0'").zero_refs.is_empty());
}

#[test]
fn quoted_heredocs_are_literal_and_plain_ones_expand() {
    let text = "cat <<'EOF'\n$(no) $0\nEOF\ncat <<EOF\n$(yes)\nEOF\necho $(after)\n";
    assert_eq!(
        substitutions(text),
        vec![("yes", true, false), ("after", false, false)]
    );
    assert!(scan(text).zero_refs.is_empty());
}

#[test]
fn a_herestring_is_not_a_heredoc() {
    let found = scan("x=$(sed 's/ *$//' <<< \"$cmd\")\necho $(after)\n");
    assert!(!found.unterminated);
    assert_eq!(found.substitutions.len(), 2);
}

#[test]
fn zero_references_keep_their_modifiers() {
    let found = scan(r#"a=$0 b=$0:h c="${0:A:h}" d=${${(%):-%x}:a} e=${(%):-%N}"#);
    let forms: Vec<ZeroForm> = found
        .zero_refs
        .iter()
        .map(|zero| zero.form.clone())
        .collect();
    assert_eq!(
        forms,
        vec![
            ZeroForm::Name {
                modifiers: String::new()
            },
            ZeroForm::Name {
                modifiers: "h".into()
            },
            ZeroForm::Name {
                modifiers: "Ah".into()
            },
            ZeroForm::Prompt { file: true },
            ZeroForm::Prompt { file: false },
        ]
    );
    assert!(found.zero_refs[3].in_param);
}

#[test]
fn zero_patterns_are_unsupported() {
    let found = scan(r#"0="${${ZERO:-${0:#$ZSH_ARGZERO}}:-${(%):-%N}}""#);
    assert_eq!(found.unsupported_zero.len(), 1);
}

#[test]
fn positional_parameters_past_zero_are_not_zero() {
    assert!(scan("echo $01 ${01}").zero_refs.is_empty());
}

#[test]
fn plain_words_are_located() {
    let text = "[[ -o interactive ]] || return 0";
    let words: Vec<&str> = scan(text)
        .words
        .iter()
        .map(|word| &text[word.range.clone()])
        .collect();
    assert!(words.ends_with(&["return", "0"]));
}

#[test]
fn an_open_quote_is_reported() {
    assert!(scan("echo \"open").unterminated);
    assert!(scan("echo $(open").unterminated);
}
