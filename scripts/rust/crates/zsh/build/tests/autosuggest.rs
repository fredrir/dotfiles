#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, tree_pairs};

/// The dotfiles' own `defer`, which runs the plugin and then its first binding.
const UTILS: &str = include_str!("../../../../../../shared/zsh/02-utils.zsh");

const RC: &str = r#"# zsh-build: omit
if [[ -z $ZSH_BUILD_SKIP && -r $HOME/.cache/build/zshrc.zsh ]]; then
  source $HOME/.cache/build/zshrc.zsh
  return
fi
source "$CONF/utils.zsh"
extra_widget() { : }
[[ -n $EXTRA_WIDGET ]] && zle -N extra-widget extra_widget
zsh_plugin_sources=("$CONF/autosuggest.zsh")
for file in $zsh_plugin_sources; do
  defer source "$file"
done
"#;

/// zsh-autosuggestions' binding, which logs each widget it binds through `eval`.
const PLUGIN: &str = r#"typeset -g ZSH_AUTOSUGGEST_ORIGINAL_WIDGET_PREFIX=autosuggest-orig-
typeset -ga ZSH_AUTOSUGGEST_CLEAR_WIDGETS=(history-search-forward)
typeset -ga ZSH_AUTOSUGGEST_IGNORE_WIDGETS=('orig-*' beep 'zle-*')
_zsh_autosuggest_invoke_original_widget() {
  (( $# )) || return 0
  local original_widget_name=$1
  shift
  (( ${+widgets[$original_widget_name]} )) && zle $original_widget_name -- $@
}
_zsh_autosuggest_widget_clear() { _zsh_autosuggest_invoke_original_widget $@ }
_zsh_autosuggest_widget_modify() { _zsh_autosuggest_invoke_original_widget $@ }
_zsh_autosuggest_incr_bind_count() {
  typeset -gi bind_count=$((_ZSH_AUTOSUGGEST_BIND_COUNTS[$1]+1))
  _ZSH_AUTOSUGGEST_BIND_COUNTS[$1]=$bind_count
}
_zsh_autosuggest_bind_widget() {
  typeset -gA _ZSH_AUTOSUGGEST_BIND_COUNTS
  local widget=$1 autosuggest_action=$2 prefix=$ZSH_AUTOSUGGEST_ORIGINAL_WIDGET_PREFIX
  local -i bind_count
  print -r -- ${ZSH_BUILD_SKIP:-bundle} >> $HOME/bind.log
  case $widgets[$widget] in
    user:_zsh_autosuggest_(bound|orig)_*)
      bind_count=$((_ZSH_AUTOSUGGEST_BIND_COUNTS[$widget]))
      ;;
    user:*)
      _zsh_autosuggest_incr_bind_count $widget
      zle -N $prefix$bind_count-$widget ${widgets[$widget]#*:}
      ;;
    builtin)
      _zsh_autosuggest_incr_bind_count $widget
      eval "_zsh_autosuggest_orig_${(q)widget}() { zle .${(q)widget} }"
      zle -N $prefix$bind_count-$widget _zsh_autosuggest_orig_$widget
      ;;
    completion:*)
      _zsh_autosuggest_incr_bind_count $widget
      eval "zle -C $prefix$bind_count-${(q)widget} ${${(s.:.)widgets[$widget]}[2,3]}"
      ;;
  esac
  eval "_zsh_autosuggest_bound_${bind_count}_${(q)widget}() {
    _zsh_autosuggest_widget_$autosuggest_action $prefix$bind_count-${(q)widget} \$@
  }"
  zle -N -- $widget _zsh_autosuggest_bound_${bind_count}_$widget
}
_zsh_autosuggest_bind_widgets() {
  emulate -L zsh
  local widget
  local ignore_widgets=(.\* _\* $ZSH_AUTOSUGGEST_ORIGINAL_WIDGET_PREFIX\* $ZSH_AUTOSUGGEST_IGNORE_WIDGETS)
  for widget in ${${(f)"$(builtin zle -la)"}:#${(j:|:)~ignore_widgets}}; do
    if [[ -n ${ZSH_AUTOSUGGEST_CLEAR_WIDGETS[(r)$widget]} ]]; then
      _zsh_autosuggest_bind_widget $widget clear
    else
      _zsh_autosuggest_bind_widget $widget modify
    fi
  done
}
_zsh_autosuggest_start() {
  add-zsh-hook -d precmd _zsh_autosuggest_start
  _zsh_autosuggest_bind_widgets
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _zsh_autosuggest_start
"#;

fn fixture() -> tempfile::TempDir {
    tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\", \"EXTRA_WIDGET\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n",
        ),
        (
            "zsh/env.zsh",
            "export CONF=\"$HOME/zsh\"\nexport DOTFILES_ZSH_CACHE=\"$HOME/.cache/zsh\"\n",
        ),
        ("zsh/rc.zsh", RC),
        ("zsh/utils.zsh", UTILS),
        ("zsh/autosuggest.zsh", PLUGIN),
        (".zshenv", "source $HOME/zsh/env.zsh\n"),
        (".zshrc", "source $HOME/zsh/rc.zsh\n"),
    ])
}

fn zsh_build(root: &Path, args: &[&str], extra: &[(&str, &str)]) -> Ran {
    let mut command = Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("ZDOTDIR")
        .env_remove("TERM_PROGRAM")
        .env_remove("EXTRA_WIDGET");
    for (name, value) in extra {
        command = command.env(name, value);
    }
    command.run()
}

fn bound(root: &Path) -> String {
    let log = root.join("bind.log");
    let text = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(log);
    text
}

fn built() -> tempfile::TempDir {
    let root = fixture();
    let built = zsh_build(root.path(), &[], &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    root
}

#[test]
fn the_first_binding_runs_as_the_code_the_build_recorded() {
    let root = built();
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(bundle.contains("_zsh_build_bound() {"), "{bundle}");
    assert!(
        bundle.contains("_zsh_autosuggest_orig_backward-char() {\n\tzle .backward-char\n}"),
        "{bundle}"
    );
    assert!(!bundle.contains(": __zb_autosuggest"), "{bundle}");
    assert!(bound(root.path()).contains("bundle"));

    let verified = zsh_build(root.path(), &["verify"], &[]);
    assert_eq!(verified.stdout, "bundle matches sources\n", "{verified:?}");
    assert!(verified.stderr.is_empty(), "{}", verified.stderr);
    let log = bound(root.path());
    assert!(log.contains('1'), "{log}");
    assert!(!log.contains("bundle"), "{log}");
}

#[test]
fn other_widgets_than_recorded_are_bound_by_the_plugin() {
    let root = built();
    bound(root.path());
    let verified = zsh_build(root.path(), &["verify"], &[("EXTRA_WIDGET", "1")]);
    assert_eq!(verified.stdout, "bundle matches sources\n", "{verified:?}");
    assert!(bound(root.path()).contains("bundle"));
}

#[test]
fn the_recorded_binding_is_kept_between_builds() {
    let root = built();
    bound(root.path());
    let again = zsh_build(root.path(), &[], &[]);
    assert!(again.stdout.contains("current"), "{again:?}");
    let dry = zsh_build(root.path(), &["--dry-run"], &[]);
    assert!(dry.stdout.contains("current"), "{dry:?}");
    assert_eq!(bound(root.path()), "");
}

#[test]
fn a_dry_run_records_nothing() {
    let root = fixture();
    let dry = zsh_build(root.path(), &["--dry-run"], &[]);
    assert!(dry.success() && dry.stderr.is_empty(), "{dry:?}");
    assert!(dry.stdout.contains("would update"), "{dry:?}");
    assert_eq!(bound(root.path()), "");
    assert!(!root.path().join(".cache/build/zshrc.zsh").exists());
}
