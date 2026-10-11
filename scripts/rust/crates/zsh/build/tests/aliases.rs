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
    let mut command = Bin::new("zsh")
        .args([
            "-fc",
            &format!("source zsh/env.zsh; source zsh/rc.zsh; {DUMP}"),
        ])
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin");
    if !bundle {
        command = command.env("NO_BUNDLE", "1");
    }
    let ran = command.run();
    assert!(ran.success(), "{ran:?}");
    ran.stdout
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
        text.contains("f_grep() { \\grep --color=auto -q x \"$@\" }"),
        "{text}"
    );
    assert!(text.contains("if \\grep --color=auto -q inner"), "{text}");

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
fn an_alias_not_known_on_every_path_is_left_and_reported() {
    let root = fixture();
    let use_file = root.path().join("zsh/use.zsh");
    let text = fs::read_to_string(&use_file).unwrap()
        + "if (( RANDOM % 2 )); then alias maybe='print -r -- yes'; fi\nf_maybe() { maybe }\n\
           named() { alias \"$1\"=\"print -r -- $2\" }\nnamed by_arg ARG\nf_by_arg() { by_arg }\n";
    fs::write(&use_file, text).unwrap();
    let built = build(root.path());
    for note in [
        "zsh/use.zsh:17: alias maybe: not known on every path; not expanded",
        "zsh/use.zsh:20: alias by_arg: not known on every path; not expanded",
    ] {
        assert!(built.stderr.contains(note), "{built:?}");
    }
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(bundle.contains("f_maybe() { maybe }"), "{bundle}");
    assert!(bundle.contains("f_by_arg() { by_arg }"), "{bundle}");
}
