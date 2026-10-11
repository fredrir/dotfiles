#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, tree_pairs};

/// The dotfiles' own `defer`, which the compiler queues deferred code through.
const UTILS: &str = include_str!("../../../../../../shared/zsh/02-utils.zsh");

const RC: &str = r#"# zsh-build: omit
if [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then
  source $HOME/.cache/build/zshrc.zsh
  return
fi
source "$CONF/utils.zsh"
source "$CONF/aliases.zsh"
source "$CONF/late.zsh"
source "$CONF/use.zsh"
"#;

const ALIASES: &str = r#"alias grep='grep --color=auto'
alias ls='ls -F'
alias ll='ls -l'
alias e='print -r -- E '
alias q='print -r -- Q'
alias -s txt='print -r -- TXT'
alias gone='print -r -- GONE'
"#;

const USE: &str = r#"f_grep() { grep -q x "$@" }
f_chain() { ll; e q; e ll }
f_suffix() { notes.txt }
f_quoted() { \grep x; 'ls'; command grep y }
if grep -q inner "$CONF/inner.zsh"; then
  source "$CONF/inner.zsh"
fi
alias x='print -r -- X'; f_same_line() { x }
setopt no_aliases
f_off() { ll }
setopt aliases
f_on() { ll }
unalias gone
f_removed() { gone }
f_late() { late }
"#;

const DUMP: &str =
    r#"for f in ${(ok)functions[(I)f_*]}; do print -r -- "$f: ${functions[$f]}"; done"#;

fn fixture() -> tempfile::TempDir {
    tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n\n[defer]\nfiles = [\"zsh/late.zsh\"]\n",
        ),
        ("zsh/env.zsh", "export CONF=\"$HOME/zsh\"\n"),
        ("zsh/rc.zsh", RC),
        ("zsh/utils.zsh", UTILS),
        ("zsh/aliases.zsh", ALIASES),
        ("zsh/late.zsh", "alias late='print -r -- LATE'\n"),
        ("zsh/use.zsh", USE),
        ("zsh/inner.zsh", "# inner\nf_inner() { ll; grep z }\n"),
    ])
}

fn build(root: &Path) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .run()
}

/// The `f_*` functions startup leaves, from the bundle or the sources.
fn functions(root: &Path, bundle: bool) -> String {
    shell(root, bundle, None, DUMP)
}

/// `script` after startup, from the bundle or the sources, in VS Code's
/// terminal when `terminal` says so.
fn shell(root: &Path, bundle: bool, terminal: Option<&str>, script: &str) -> String {
    let ran = run(root, bundle, terminal, script);
    assert!(ran.success(), "{ran:?}");
    ran.stdout
}

fn run(root: &Path, bundle: bool, terminal: Option<&str>, script: &str) -> Ran {
    let mut command = Bin::new("zsh")
        .args([
            "-fc",
            &format!("source zsh/env.zsh; source zsh/rc.zsh; {script}"),
        ])
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("TERM_PROGRAM");
    if let Some(terminal) = terminal {
        command = command.env("TERM_PROGRAM", terminal);
    }
    if !bundle {
        command = command.env("NO_BUNDLE", "1");
    }
    command.run()
}

/// `text` without a backslash before a plain word, which the build adds so a
/// bundle read as text expands no alias twice; a parsed body runs the same.
fn unquoted(text: &str) -> String {
    let bytes = text.as_bytes();
    text.char_indices()
        .filter(|(index, character)| {
            let starts = *index == 0 || b" \t\n;|&({".contains(&bytes[index - 1]);
            let plain = bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'_');
            !(*character == '\\' && starts && plain)
        })
        .map(|(_, character)| character)
        .collect()
}

#[test]
fn a_bundle_from_wordcode_or_read_as_text_expands_aliases_as_the_sources() {
    let root = fixture();
    let built = build(root.path());
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let text = fs::read_to_string(&bundle).unwrap();
    assert!(text.contains("__zb_defer_1() {"), "{text}");
    assert!(
        text.contains("f_grep() { grep --color=auto -q x \"$@\" }"),
        "{text}"
    );
    assert!(text.contains("if grep --color=auto -q inner"), "{text}");

    let sources = functions(root.path(), false);
    assert!(
        sources.contains("f_chain: \tls -F -l\n\tprint -r -- E print -r -- Q\n"),
        "{sources}"
    );
    assert!(
        sources.contains("f_late: \tprint -r -- LATE\n"),
        "{sources}"
    );
    let wordcode = functions(root.path(), true);
    assert_eq!(unquoted(&wordcode), unquoted(&sources));

    let compiled = bundle.with_extension("zsh.zwc");
    fs::remove_file(&compiled).unwrap();
    let read_as_text = functions(root.path(), true);
    assert_eq!(read_as_text, wordcode);
}

#[test]
fn a_function_named_after_an_alias_removed_before_it_is_defined_read_as_text_too() {
    let root = fixture();
    let aliases = root.path().join("zsh/aliases.zsh");
    let text = fs::read_to_string(&aliases).unwrap() + "alias gx='print -r -- aliased'\n";
    fs::write(&aliases, text).unwrap();
    let use_file = root.path().join("zsh/use.zsh");
    let text =
        fs::read_to_string(&use_file).unwrap() + "unalias gx\ngx() { print -r -- function }\n";
    fs::write(&use_file, text).unwrap();
    let built = build(root.path());
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");

    let sources = run(root.path(), false, None, "gx");
    let wordcode = run(root.path(), true, None, "gx");
    fs::remove_file(root.path().join(".cache/build/zshrc.zsh.zwc")).unwrap();
    let read_as_text = run(root.path(), true, None, "gx");
    for ran in [sources, wordcode, read_as_text] {
        assert_eq!(ran.stdout, "function\n", "{ran:?}");
        assert!(ran.stderr.is_empty(), "{ran:?}");
    }
}

#[test]
fn a_statement_whose_alias_differs_between_paths_is_parsed_when_it_runs_or_reported() {
    let root = fixture();
    let use_file = root.path().join("zsh/use.zsh");
    let text = fs::read_to_string(&use_file).unwrap()
        + "if (( RANDOM % 2 )); then alias maybe='print -r -- yes'; fi\nf_maybe() { maybe }\n\
           named() { alias \"$1\"=\"print -r -- $2\" }\nnamed by_arg ARG\nf_by_arg() { by_arg }\n\
           f_zero() { maybe; print -r -- $0 }\n";
    fs::write(&use_file, text).unwrap();
    let built = build(root.path());
    assert!(
        built
            .stderr
            .contains("zsh/use.zsh:21: alias maybe: not known on every path; not expanded"),
        "{built:?}"
    );
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    for runtime in [
        "\\builtin eval '{ f_maybe() { maybe }\n} always",
        "\\builtin eval '{ f_by_arg() { by_arg }\n} always",
    ] {
        assert!(bundle.contains(runtime), "{bundle}");
    }
    assert!(
        bundle.contains("\nf_zero() { maybe; print -r -- $0 }"),
        "{bundle}"
    );
}

const PATHS_RC: &str = r#"# zsh-build: omit
if [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then
  source $HOME/.cache/build/zshrc.zsh
  return
fi
source "$CONF/utils.zsh"
source "$CONF/aliases.zsh"
source "$CONF/late.zsh"
source "$CONF/use.zsh"
"#;

/// Aliases set only outside VS Code's terminal, as `30-aliases.zsh` sets them.
fn paths_fixture(late: &str, use_file: &str) -> tempfile::TempDir {
    tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n\n[defer]\nfiles = [\"zsh/late.zsh\"]\n",
        ),
        ("zsh/env.zsh", "export CONF=\"$HOME/zsh\"\n"),
        ("zsh/rc.zsh", PATHS_RC),
        ("zsh/utils.zsh", UTILS),
        (
            "zsh/aliases.zsh",
            "[[ $TERM_PROGRAM == vscode ]] && return 0\nalias grep='grep --color=auto'\nalias ll='ls -l'\n",
        ),
        ("zsh/late.zsh", late),
        ("zsh/use.zsh", use_file),
    ])
}

#[test]
fn a_function_using_an_alias_set_on_one_path_matches_the_sources_on_both() {
    let root = paths_fixture(
        "alias late='print -r -- LATE'\n",
        "f_grep() { grep -q x \"$@\" }\nf_ll() { ll }\nf_plain() { print -r -- plain }\n",
    );
    let built = build(root.path());
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let text = fs::read_to_string(&bundle).unwrap();
    assert!(
        text.contains("\\builtin eval '{ f_grep() { grep -q x \"$@\" }\n} always"),
        "{text}"
    );
    assert!(
        text.contains("\\builtin eval '{ f_ll() { ll }\n} always"),
        "{text}"
    );
    assert!(text.contains("\nf_plain() { print -r -- plain }"), "{text}");
    assert!(text.contains("__zb_defer_1() {"), "{text}");

    let typed = shell(root.path(), true, None, "type f_ll");
    let origin = root.path().join("zsh/use.zsh");
    assert_eq!(
        typed,
        format!("f_ll is a shell function from {}:2\n", origin.display())
    );

    let paths = [None, Some("vscode")];
    let sources: Vec<String> = paths
        .iter()
        .map(|terminal| shell(root.path(), false, *terminal, DUMP))
        .collect();
    assert!(sources[0].contains("f_ll: \tls -l\n"), "{}", sources[0]);
    assert!(sources[1].contains("f_ll: \tll\n"), "{}", sources[1]);
    for (terminal, sources) in paths.iter().zip(&sources) {
        assert_eq!(
            &shell(root.path(), true, *terminal, DUMP),
            sources,
            "{terminal:?}"
        );
    }
    fs::remove_file(bundle.with_extension("zsh.zwc")).unwrap();
    for (terminal, sources) in paths.iter().zip(&sources) {
        assert_eq!(
            &shell(root.path(), true, *terminal, DUMP),
            sources,
            "text {terminal:?}"
        );
    }
}

#[test]
fn deferred_code_setting_an_alias_a_statement_parsed_when_it_runs_uses_stays_in_place() {
    let root = paths_fixture(
        "[[ $TERM_PROGRAM == vscode ]] || alias maybe='print -r -- MAYBE'\n",
        "f_maybe() { maybe }\n",
    );
    let built = build(root.path());
    let late = root.path().join("zsh/late.zsh");
    assert!(
        built.stderr.contains(&format!(
            "{}: alias maybe changed later; not deferred",
            late.display()
        )),
        "{built:?}"
    );
    let text = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(!text.contains("__zb_defer_"), "{text}");
    assert!(
        text.contains("\\builtin eval '{ f_maybe() { maybe }\n} always"),
        "{text}"
    );
    for terminal in [None, Some("vscode")] {
        assert_eq!(
            shell(root.path(), true, terminal, DUMP),
            shell(root.path(), false, terminal, DUMP),
            "{terminal:?}"
        );
    }
}

#[test]
fn a_statement_parsed_when_it_runs_stops_its_file_on_an_error_as_in_the_sources() {
    let root = paths_fixture(
        "alias late='print -r -- LATE'\n",
        "before=1\nll -d / >/dev/null 2>&1 && : $(( 1 / 0 ))\nstatement=$?\nafter=1\n",
    );
    let built = build(root.path());
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let text = fs::read_to_string(&bundle).unwrap();
    assert!(
        text.contains("\\builtin eval '{ ll -d / >/dev/null 2>&1 && : $(( 1 / 0 ))\n} always")
            && text.contains("_zsh_build_raise\n"),
        "{text}"
    );
    let report = r#"print -r -- "before=$before statement=${statement-unset} after=${after-unset} stopped=${_zsh_build_stopped-unset}""#;
    let expected = [
        (None, "before=1 statement=unset after=unset stopped=unset\n"),
        (
            Some("vscode"),
            "before=1 statement=127 after=1 stopped=unset\n",
        ),
    ];
    for (terminal, report_text) in expected {
        assert_eq!(shell(root.path(), false, terminal, report), report_text);
        assert_eq!(shell(root.path(), true, terminal, report), report_text);
    }
    fs::remove_file(bundle.with_extension("zsh.zwc")).unwrap();
    for (terminal, report_text) in expected {
        assert_eq!(
            shell(root.path(), true, terminal, report),
            report_text,
            "text {terminal:?}"
        );
    }
}
