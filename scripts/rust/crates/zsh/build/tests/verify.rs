#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, executable, tree_pairs};

/// The dotfiles' own `defer`, which the compiler queues deferred code through.
const UTILS: &str = include_str!("../../../../../../shared/zsh/02-utils.zsh");

const RC: &str = r#"# zsh-build: omit
if [[ -z $ZSH_BUILD_SKIP && -r $HOME/.cache/build/zshrc.zsh ]]; then
  source $HOME/.cache/build/zshrc.zsh
  return
fi
source "$CONF/utils.zsh"
add_path "$HOME/bin"
alias ll='ls -l'
setup() {
  local ready=1
  eval "$(init --zsh)"
}
frames() {
  eval "$(init --frames)"
}
setup
source "$CONF/late.zsh"
"#;

const INIT: &str = "#!/bin/sh\ncase $1 in\n  --zsh) printf 'init_fn() { print -r -- initialized }\\ninit_ready=1\\n' ;;\n  --frames) printf 'init_frames=${#funcstack}\\n' ;;\nesac\n";

fn fixture() -> tempfile::TempDir {
    let root = tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n\n[defer]\nfiles = [\"zsh/late.zsh\"]\n\n[fold]\ncommands = [\"init --zsh\", \"init --frames\"]\n",
        ),
        (
            "zsh/env.zsh",
            "export CONF=\"$HOME/zsh\"\nexport DOTFILES_ZSH_CACHE=\"$HOME/.cache/zsh\"\n",
        ),
        ("zsh/rc.zsh", RC),
        ("zsh/utils.zsh", UTILS),
        (
            "zsh/late.zsh",
            "late_fn() { print late }\nbindkey '^Xl' beginning-of-line\nzstyle ':late:*' mode on\n",
        ),
        (".zshenv", "source $HOME/zsh/env.zsh\n"),
        (".zshrc", "source $HOME/zsh/rc.zsh\n"),
    ]);
    fs::create_dir_all(root.path().join("bin")).unwrap();
    executable(&root.path().join("bin/init"), INIT);
    root
}

fn zsh_build(root: &Path, args: &[&str]) -> Ran {
    in_terminal(root, args, None)
}

/// `zsh-build` run from a terminal that sets `TERM_PROGRAM`, if any.
fn in_terminal(root: &Path, args: &[&str], terminal: Option<&str>) -> Ran {
    let command = Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("HOME", root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env_remove("ZDOTDIR")
        .env_remove("TERM_PROGRAM");
    match terminal {
        Some(terminal) => command.env("TERM_PROGRAM", terminal).run(),
        None => command.run(),
    }
}

fn built() -> tempfile::TempDir {
    let root = fixture();
    let built = zsh_build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    root
}

fn bundle(root: &Path) -> String {
    fs::read_to_string(root.join(".cache/build/zshrc.zsh")).unwrap()
}

fn append(path: &Path, text: &str) {
    let text = fs::read_to_string(path).unwrap() + text;
    fs::write(path, text).unwrap();
}

#[test]
fn a_bundle_that_rewrites_function_bodies_matches_its_sources() {
    let root = built();
    let text = bundle(root.path());
    assert!(text.contains("__zb_defer_1() {"), "{text}");
    assert!(text.contains("eval \"${__zb"), "{text}");
    assert!(text.contains("  {\ninit_fn() {"), "{text}");
    let verified = zsh_build(root.path(), &["verify"]);
    assert!(verified.success(), "{verified:?}");
    assert_eq!(verified.stdout, "bundle matches sources\n");
    assert!(verified.stderr.is_empty(), "{}", verified.stderr);
}

#[test]
fn an_alias_the_build_expands_in_a_function_matches() {
    let root = fixture();
    let rc = root.path().join("zsh/rc.zsh");
    let text = fs::read_to_string(&rc).unwrap().replace(
        "alias ll='ls -l'\n",
        "alias ls='ls -F'\nalias ll='ls -l'\nlsl() { ll \"$@\" }\n",
    );
    fs::write(&rc, text).unwrap();
    assert!(zsh_build(root.path(), &[]).success());
    assert!(bundle(root.path()).contains("lsl() { ls -F -l \"$@\" }"));
    let verified = zsh_build(root.path(), &["verify"]);
    assert!(verified.success(), "{verified:?}");
    assert_eq!(verified.stdout, "bundle matches sources\n");
}

#[test]
fn an_alias_set_on_one_path_matches_on_both() {
    let root = fixture();
    let rc = root.path().join("zsh/rc.zsh");
    let text = fs::read_to_string(&rc).unwrap().replace(
        "alias ll='ls -l'\n",
        "source \"$CONF/aliases.zsh\"\nlsl() { ll \"$@\" }\n",
    );
    fs::write(&rc, text).unwrap();
    fs::write(
        root.path().join("zsh/aliases.zsh"),
        "[[ $TERM_PROGRAM == vscode ]] && return 0\nalias ll='ls -l'\n",
    )
    .unwrap();
    let built = zsh_build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    assert!(bundle(root.path()).contains("\\builtin eval '{ lsl() { ll \"$@\" }\n} always"));
    for terminal in [None, Some("vscode")] {
        let verified = in_terminal(root.path(), &["verify"], terminal);
        assert!(verified.success(), "{terminal:?} {verified:?}");
        assert_eq!(verified.stdout, "bundle matches sources\n");
    }
}

#[test]
fn an_alias_the_build_leaves_in_a_function_is_reported() {
    let root = fixture();
    let rc = root.path().join("zsh/rc.zsh");
    let text = fs::read_to_string(&rc).unwrap().replace(
        "alias ll='ls -l'\n",
        "alias ll='ls -l | cat'\nlsl() { ll \"$@\" }\n",
    );
    fs::write(&rc, text).unwrap();
    let built = zsh_build(root.path(), &[]);
    assert!(
        built
            .stderr
            .contains("alias ll: value is not one command of plain words; not expanded"),
        "{built:?}"
    );
    let verified = zsh_build(root.path(), &["verify"]);
    assert_eq!(verified.code(), Some(1), "{verified:?}");
    assert_eq!(
        verified.stdout,
        "functions\n  lsl  bundle: ll \"$@\"  sources: ls -l | cat \"$@\"\n"
    );
}

#[test]
fn a_stale_bundle_is_reported_and_left_as_built() {
    let root = built();
    let before = bundle(root.path());
    append(
        &root.path().join("zsh/rc.zsh"),
        "alias extra='print extra'\n",
    );
    let verified = zsh_build(root.path(), &["verify"]);
    assert_eq!(verified.code(), Some(1), "{verified:?}");
    assert!(
        verified.stderr.contains("zshrc.zsh: stale"),
        "{}",
        verified.stderr
    );
    assert!(
        verified.stdout.contains("aliases\n  extra  bundle: (none)"),
        "{}",
        verified.stdout
    );
    assert_eq!(bundle(root.path()), before);
}

#[test]
fn stale_wordcode_stops_verify_before_a_shell_recompiles_it() {
    let root = built();
    let wordcode = root.path().join(".cache/build/zshrc.zsh.zwc");
    let compiled = fs::read(&wordcode).unwrap();
    append(&root.path().join(".cache/build/zshrc.zsh"), "\n");
    let verified = zsh_build(root.path(), &["verify"]);
    assert!(!verified.success());
    assert!(
        verified.stderr.contains("zshrc.zsh.zwc: stale"),
        "{}",
        verified.stderr
    );
    assert_eq!(fs::read(&wordcode).unwrap(), compiled);
}
