#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, tree_pairs};

/// oh-my-zsh's completion setup, verbatim from oh-my-zsh.sh.
const SECTION: &str = r##"# Check whether the zcompdump file carries the current OMZ metadata lines
_omz_compdump_has_metadata() {
  local -a lines
  [[ -r "$ZSH_COMPDUMP" ]] || return 1
  lines=("${(@f)$(<"$ZSH_COMPDUMP")}")
  (( ${lines[(Ie)$zcompdump_revision]} && ${lines[(Ie)$zcompdump_fpath]} ))
}

# Delete the zcompdump file if OMZ zcompdump metadata changed
zcompdump_refresh=0
if ! _omz_compdump_has_metadata; then
  command rm -f "$ZSH_COMPDUMP"
  zcompdump_refresh=1
fi

if [[ "$ZSH_DISABLE_COMPFIX" != true ]]; then
  source "$ZSH/lib/compfix.zsh"
  # Load only from secure directories
  # Reset the flag compinit sets when -i excludes insecure entries
  unset _comp_secure
  compinit -i -d "$ZSH_COMPDUMP"
  # If completion insecurities exist, warn the user
  [[ "$_comp_secure" == yes ]] && handle_completion_insecurities &|
else
  # If the user wants it, load from all found directories
  compinit -u -d "$ZSH_COMPDUMP"
fi

# Append zcompdump metadata if missing (compinit may have regenerated the file)
if (( zcompdump_refresh )) || ! _omz_compdump_has_metadata; then
  # Use `tee` in case the $ZSH_COMPDUMP filename is invalid, to silence the error
  # See https://github.com/ohmyzsh/ohmyzsh/commit/dd1a7269#commitcomment-39003489
  tee -a "$ZSH_COMPDUMP" &>/dev/null <<EOF

$zcompdump_revision
$zcompdump_fpath
EOF
fi
unset zcompdump_revision zcompdump_fpath zcompdump_refresh
unset -f _omz_compdump_has_metadata

# zcompile the completion dump file if the .zwc is older or missing.
# Test that first so the lock directory and zrecompile are skipped when it's fresh.
if [[ ! "${ZSH_COMPDUMP}.zwc" -nt "$ZSH_COMPDUMP" ]] \
   && command mkdir "${ZSH_COMPDUMP}.lock" 2>/dev/null; then
  zrecompile -q -p "$ZSH_COMPDUMP"
  command rm -rf "$ZSH_COMPDUMP.zwc.old" "${ZSH_COMPDUMP}.lock"
fi

"##;

const PREAMBLE: &str = r##"fpath=("$HOME/completions" $fpath)
autoload -U compaudit compinit zrecompile
compaudit() { print -r -- audit >> "$HOME/audit.log"; return 0 }
ZSH_DISABLE_COMPFIX=true
ZSH_COMPDUMP="$HOME/.zcompdump-test"
zcompdump_revision="#omz revision: test"
zcompdump_fpath="#omz fpath: $fpath"
"##;

fn fixture() -> tempfile::TempDir {
    let omz = format!("{PREAMBLE}{SECTION}");
    tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\n",
        ),
        (
            "zsh/rc.zsh",
            "# zsh-build: omit\nif [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then\n  source $HOME/.cache/build/zshrc.zsh\n  return\nfi\nsource \"$HOME/zsh/omz.zsh\"\n",
        ),
        ("zsh/omz.zsh", &omz),
        (
            "completions/_fixturecmd",
            "#compdef fixturecmd\n_message fixture\n",
        ),
    ])
}

fn build(root: &Path) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .env("HOME", root)
        .env_remove("ZDOTDIR")
        .run()
}

/// The completion a command gets, and how many audits have run so far.
fn startup(root: &Path, bundle: bool) -> (String, usize) {
    let mut command = Bin::new("zsh")
        .args([
            "-fc",
            "source zsh/rc.zsh; print -r -- ${_comps[fixturecmd]}:${_comps[newcmd]}",
        ])
        .current_dir(root)
        .env("HOME", root);
    if !bundle {
        command = command.env("NO_BUNDLE", "1");
    }
    let ran = command.run();
    assert!(ran.success(), "{ran:?}");
    let audits = fs::read_to_string(root.join("audit.log"))
        .unwrap_or_default()
        .lines()
        .count();
    (ran.stdout.trim().to_string(), audits)
}

#[test]
fn a_current_dump_is_loaded_without_an_audit() {
    let root = fixture();
    let built = build(root.path());
    assert!(built.success(), "{built:?}");
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(bundle.contains("compinit -C -d"), "{bundle}");

    assert_eq!(startup(root.path(), true), ("_fixturecmd:".to_string(), 1));
    assert!(root.path().join(".zcompdump-test.fpath").is_file());
    assert_eq!(startup(root.path(), true), ("_fixturecmd:".to_string(), 1));
    assert_eq!(startup(root.path(), false).0, "_fixturecmd:");
}

#[test]
fn a_new_completion_function_appears_after_the_next_build() {
    let root = fixture();
    assert!(build(root.path()).success());
    startup(root.path(), true);
    fs::write(
        root.path().join("completions/_newcmd"),
        "#compdef newcmd\n_message new\n",
    )
    .unwrap();
    assert_eq!(startup(root.path(), true).0, "_fixturecmd:");

    let rebuilt = build(root.path());
    assert!(
        rebuilt.stdout.contains("completion dump refreshed"),
        "{rebuilt:?}"
    );
    assert!(!root.path().join(".zcompdump-test").exists());
    assert_eq!(startup(root.path(), true).0, "_fixturecmd:_newcmd");
}

#[test]
fn a_changed_fpath_takes_the_full_path() {
    let root = fixture();
    assert!(build(root.path()).success());
    let (_, audits) = startup(root.path(), true);
    fs::write(root.path().join(".zcompdump-test.fpath"), "/elsewhere\n").unwrap();
    assert_eq!(
        startup(root.path(), true),
        ("_fixturecmd:".to_string(), audits + 1)
    );
}
