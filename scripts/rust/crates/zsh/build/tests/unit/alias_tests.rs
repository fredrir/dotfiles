use super::*;

fn table(
    aliases: &[(&str, &str)],
    globals: &[(&str, &str)],
    suffixes: &[(&str, &str)],
) -> (Table, Seen) {
    let mut table = Table::default();
    let mut seen = Seen::default();
    for (kind, pairs) in [
        (Kind::Regular, aliases),
        (Kind::Global, globals),
        (Kind::Suffix, suffixes),
    ] {
        for (name, text) in pairs {
            table.define(kind, name, Some(text.to_string()), &mut seen);
        }
    }
    (table, seen)
}

fn command_words(text: &str) -> Vec<(&str, bool)> {
    words(text)
        .unwrap()
        .into_iter()
        .filter(|word| !word.function)
        .map(|word| (&text[word.range], word.command))
        .collect()
}

fn expanded(text: &str, table: &Table, seen: &Seen) -> Result<(String, String), Left> {
    let found = plan(text, &words(text).unwrap(), table, seen)?;
    Ok((apply(text, &found, false), apply(text, &found, true)))
}

#[test]
fn command_position_follows_zsh() {
    assert_eq!(
        command_words("a | b && c; d & e"),
        [
            ("a", true),
            ("b", true),
            ("c", true),
            ("d", true),
            ("e", true)
        ]
    );
    assert_eq!(
        command_words("if a; then b x; fi; ! c; time d; nocorrect e; coproc f"),
        [
            ("a", true),
            ("b", true),
            ("x", false),
            ("c", true),
            ("d", true),
            ("e", true),
            ("f", true)
        ]
    );
    assert_eq!(
        command_words("command a; builtin b; noglob c; exec d"),
        [
            ("command", true),
            ("a", false),
            ("builtin", true),
            ("b", false),
            ("noglob", true),
            ("c", false),
            ("exec", true),
            ("d", false)
        ]
    );
    assert_eq!(command_words("X=1 a > out b"), [("a", true), ("b", false)]);
    assert_eq!(
        command_words("f() { a; }; function g { b }; () { c }"),
        [("a", true), ("b", true), ("c", true)]
    );
    let named: Vec<bool> = words("f () { a }")
        .unwrap()
        .iter()
        .map(|word| word.function)
        .collect();
    assert_eq!(named, [true, false]);
    assert_eq!(
        command_words("for x in a b; do c; done; case $y in (p|q) d ;; r) e ;; esac"),
        [
            ("a", false),
            ("b", false),
            ("c", true),
            ("$y", false),
            ("p", false),
            ("q", false),
            ("d", true),
            ("r", false),
            ("e", true)
        ]
    );
    assert_eq!(
        command_words("[[ a == b ]] && c $(d) \"e\" 'f' `g`"),
        [
            ("a", false),
            ("==", false),
            ("b", false),
            ("c", true),
            ("$(d)", false),
            ("\"e\"", false),
            ("'f'", false),
            ("`g`", false)
        ]
    );
    assert_eq!(
        command_words("cat <<EOF\nnot words\nEOF\nb"),
        [("cat", true), ("b", true)]
    );
    assert_eq!(
        command_words("(( x++ )) && a; repeat 2 do b; done"),
        [("a", true), ("b", true)]
    );
}

#[test]
fn quoted_words_are_not_plain() {
    let plain: Vec<bool> = words("a \\b 'c' d\"\" e$f g*")
        .unwrap()
        .into_iter()
        .map(|word| word.plain)
        .collect();
    assert_eq!(plain, [true, false, false, false, false, false]);
    assert!(words("print 'unterminated").is_none());
}

#[test]
fn aliases_expand_as_zsh_reads_them() {
    let (table, seen) = table(
        &[
            ("grep", "grep --color=auto"),
            ("ls", "ls -G"),
            ("ll", "ls -l"),
            ("e", "print E "),
            ("q", "print Q"),
        ],
        &[("G", "wc -l")],
        &[("txt", "less")],
    );
    let expand = |text| expanded(text, &table, &seen).unwrap();
    assert_eq!(
        expand("grep x"),
        ("grep --color=auto x".into(), "\\grep --color=auto x".into())
    );
    assert_eq!(expand("ll"), ("ls -G -l".into(), "\\ls -G -l".into()));
    assert_eq!(
        expand("e q"),
        ("print E  print Q".into(), "print E  print Q".into())
    );
    assert_eq!(
        expand("print q G"),
        ("print q wc -l".into(), "print q wc -l".into())
    );
    assert_eq!(
        expand("notes.txt"),
        ("less notes.txt".into(), "less notes.txt".into())
    );
    assert_eq!(
        expand("command grep x"),
        ("command grep x".into(), "command grep x".into())
    );
    assert_eq!(
        expand("\\grep x; 'grep' y"),
        ("\\grep x; 'grep' y".into(), "\\grep x; 'grep' y".into())
    );
}

#[test]
fn a_word_a_bundle_read_as_text_could_expand_is_quoted() {
    let (mut table, mut seen) = table(&[("x", "print X")], &[], &[]);
    table.remove(false, "x");
    let (expanded, protected) = expanded("x; print x", &table, &seen).unwrap();
    assert_eq!(
        (expanded.as_str(), protected.as_str()),
        ("x; print x", "\\x; print x")
    );
    table.define(Kind::Global, "y", Some("why".into()), &mut seen);
    table.remove(false, "y");
    assert_eq!(expanded_protected("print y", &table, &seen), "print \\y");
}

fn expanded_protected(text: &str, table: &Table, seen: &Seen) -> String {
    expanded(text, table, seen).unwrap().1
}

#[test]
fn what_differs_between_paths_or_is_no_plain_command_is_left() {
    let (mut table, mut seen) = table(
        &[
            ("a", "print A"),
            ("pipe", "ls | cat"),
            ("c", "nocorrect cd"),
        ],
        &[],
        &[],
    );
    let mut other = table.clone();
    other.define(Kind::Regular, "a", Some("print B".into()), &mut seen);
    table.merge(&other);
    let reason = |text, table: &Table| expanded(text, table, &seen).unwrap_err().reason;
    assert_eq!(reason("a", &table), "not known on every path");
    assert_eq!(
        reason("pipe", &table),
        "value is not one command of plain words"
    );
    assert_eq!(
        reason("c", &table),
        "value is not one command of plain words"
    );
    assert_eq!(expanded_protected("print a", &table, &seen), "print a");
    table.option = None;
    assert_eq!(
        reason("pipe", &table),
        "aliases option not known on every path"
    );
    table.option = Some(false);
    assert_eq!(expanded_protected("pipe", &table, &seen), "\\pipe");
}

#[test]
fn a_name_set_on_one_path_keeps_its_kind() {
    let (mut table, mut seen) = table(&[], &[], &[]);
    let mut other = table.clone();
    other.define(Kind::Regular, "r", Some("x".into()), &mut seen);
    table.merge(&other);
    assert!(expanded("print r", &table, &seen).is_ok());
    assert!(expanded("r", &table, &seen).is_err());
    table.forget("u", &mut seen);
    assert!(expanded("print u", &table, &seen).is_err());
}
