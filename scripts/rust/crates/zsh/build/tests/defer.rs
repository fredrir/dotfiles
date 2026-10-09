#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use testkit::{Bin, Ran, executable, pty, tree_pairs};

/// The dotfiles' own `defer`, which the compiler queues deferred code through.
const UTILS: &str = include_str!("../../../../../../shared/zsh/02-utils.zsh");

const OMZ: &str = r#"autoload -U compaudit compinit zrecompile
if [[ -z "$ZSH_COMPDUMP" ]]; then
  ZSH_COMPDUMP="$HOME/.zcompdump"
fi
mark compinit
[[ -n $SLOW_COMPINIT ]] && sleep $SLOW_COMPINIT
compinit -u -d "$ZSH_COMPDUMP"
_omz_source() {
  local context filepath="$1"
  case "$filepath" in
  lib/*) context="lib:${filepath:t:r}" ;;
  plugins/*) context="plugins:${filepath:h:t}" ;;
  esac
  local disable_aliases=0
  zstyle -T ":omz:${context}" aliases || disable_aliases=1
  local -A aliases_pre galiases_pre
  if (( disable_aliases )); then
    aliases_pre=("${(@kv)aliases}")
    galiases_pre=("${(@kv)galiases}")
  fi
  if [[ -f "$ZSH_CUSTOM/$filepath" ]]; then
    source "$ZSH_CUSTOM/$filepath"
  elif [[ -f "$ZSH/$filepath" ]]; then
    source "$ZSH/$filepath"
  fi
  if (( disable_aliases )); then
    if (( #aliases_pre )); then
      aliases=("${(@kv)aliases_pre}")
    else
      (( #aliases )) && unalias "${(@k)aliases}"
    fi
    if (( #galiases_pre )); then
      galiases=("${(@kv)galiases_pre}")
    else
      (( #galiases )) && unalias "${(@k)galiases}"
    fi
  fi
}
for plugin ($plugins); do
  _omz_source "plugins/$plugin/$plugin.plugin.zsh"
done
unset plugin
"#;

const RC: &str = r#"# zsh-build: omit
if [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then
  source $HOME/.cache/build/zshrc.zsh
  return
fi
source "$CONF/utils.zsh"
mark() { print -r -- $1 >> $HOME/order.log }
plugins=(alpha beta)
source "$ZSH/oh-my-zsh.sh"
mark main
cached_eval tool-completion tool --completions zsh
source "$CONF/late.zsh"
source "$CONF/locals.zsh"
source "$CONF/after.zsh"
autoload -Uz add-zle-hook-widget
_line_init_mark() { print -r -- ok >| $HOME/line-init; add-zle-hook-widget -d line-init _line_init_mark }
add-zle-hook-widget line-init _line_init_mark
[[ -n $SLOW_START ]] && sleep $SLOW_START
_first() { mark prompt; precmd_functions=(${precmd_functions:#_first}) }
precmd_functions+=(_first)
PROMPT='READY> '
"#;

/// What ran in which order, logged to a file the compiler does not track, and
/// what startup left defined.
const REPORT: &str = r#"print -r -- "order=${(j: :)${(f)"$(<$HOME/order.log)"}} alias=$+aliases[al] fns=$+functions[alpha_fn]$+functions[tool_fn]$+functions[late_fn] hidden=${hidden-unset} locals=${locals_var-unset} comp=$_comps[shared-cmd] left=${(k)functions[(I)__zb_*]}${+__zb_fpath}" >| $HOME/report"#;

const TOOL: &str = "#!/bin/sh\nprintf 'mark tool\\ntool_fn() { print tool }\\n'\n";

fn fixture() -> tempfile::TempDir {
    let root = tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\", \"ZSH_CUSTOM\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n\n[defer]\ncompinit = true\nplugins = [\"alpha\"]\nevals = [\"tool-*\"]\nfiles = [\"zsh/late.zsh\", \"zsh/locals.zsh\"]\n\n[fold]\ncommands = []\n",
        ),
        (
            "zsh/env.zsh",
            "export CONF=\"$HOME/zsh\"\nexport ZSH=\"$HOME/omz\"\nexport DOTFILES_ZSH_CACHE=\"$HOME/.cache/zsh\"\n",
        ),
        ("zsh/rc.zsh", RC),
        ("zsh/utils.zsh", UTILS),
        ("zsh/late.zsh", "mark late\nlate_fn() { print late }\n"),
        ("zsh/locals.zsh", "typeset locals_var=kept\nmark locals\n"),
        (
            "zsh/after.zsh",
            "mark after\n_after() { : }\ncompdef _after shared-cmd\ndefer 'mark queued'\n",
        ),
        ("omz/oh-my-zsh.sh", OMZ),
        (
            "omz/plugins/alpha/alpha.plugin.zsh",
            "mark alpha\nlocal hidden=1\nalias al='print alpha'\nalpha_fn() { print alpha }\n_alpha() { : }\ncompdef _alpha shared-cmd\n_alpha_widget() { BUFFER='mark widget' }\nzle -N alpha-widget _alpha_widget\nbindkey '^Xa' alpha-widget\n",
        ),
        ("omz/plugins/beta/beta.plugin.zsh", "mark beta\n"),
        (".zshenv", "source $HOME/zsh/env.zsh\n"),
        (
            ".zshrc",
            "HISTFILE=$HOME/.history\nsource $HOME/zsh/rc.zsh\nHISTFILE=$HOME/.history\n",
        ),
    ]);
    fs::create_dir_all(root.path().join("bin")).unwrap();
    executable(&root.path().join("bin/tool"), TOOL);
    root
}

fn path(root: &Path) -> String {
    format!("{}:/usr/bin:/bin", root.join("bin").display())
}

fn build(root: &Path) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .env("HOME", root)
        .env("PATH", path(root))
        .env_remove("ZDOTDIR")
        .env_remove("ZSH_CUSTOM")
        .run()
}

/// An interactive zsh on a pty, with its rc files and history in `root`.
struct Shell {
    child: Child,
    master: File,
    output: Vec<u8>,
}

impl Shell {
    fn spawn(root: &Path, args: &[&str], extra: &[(&str, &str)]) -> Shell {
        let (master, slave, _) = pty::open_pty(24, 160);
        let (stdin, stdout, stderr) = pty::stdio(&slave);
        let mut command = Command::new("zsh");
        command
            .args(args)
            .current_dir(root)
            .env_clear()
            .env("HOME", root)
            .env("ZDOTDIR", root)
            .env("HISTFILE", root.join(".history"))
            .env("PATH", path(root))
            .env("TERM", "xterm")
            .envs(extra.iter().copied())
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr);
        pty::take_controlling_terminal(&mut command);
        let child = command.spawn().unwrap();
        drop(slave);
        Shell {
            child,
            master,
            output: Vec::new(),
        }
    }

    fn wait_until(&mut self, done: impl Fn(&[u8]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(&self.output) {
            assert!(
                Instant::now() < deadline,
                "timed out: {}",
                String::from_utf8_lossy(&self.output)
            );
            pty::read_available(&self.master, &mut self.output, 20);
        }
    }

    fn type_keys(&mut self, keys: &str) {
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    /// Waits for `file`, reading the pty so the shell never blocks on output.
    fn wait_for_file(&mut self, file: &Path) -> String {
        self.wait_until(|_| file.exists() && fs::metadata(file).is_ok_and(|meta| meta.len() > 0));
        std::thread::sleep(Duration::from_millis(50));
        fs::read_to_string(file).unwrap()
    }

    /// Runs until the shell exits on its own.
    fn finish(mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "shell did not exit");
            pty::read_available(&self.master, &mut self.output, 20);
        }
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn prompt(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).contains("READY> ")
}

fn report(root: &Path) -> PathBuf {
    root.join("report")
}

fn built() -> tempfile::TempDir {
    let root = fixture();
    let built = build(root.path());
    assert!(built.success(), "{built:?}");
    assert!(built.stdout.contains("4 deferred"), "{built:?}");
    root
}

#[test]
fn deferred_code_runs_after_the_first_prompt_in_its_original_order() {
    let root = built();
    let mut shell = Shell::spawn(root.path(), &["-di"], &[]);
    shell.wait_until(prompt);
    shell.type_keys(&format!("{REPORT}\r"));
    let report = shell.wait_for_file(&report(root.path()));
    assert_eq!(
        report.trim(),
        "order=beta main locals after prompt compinit alpha tool late queued alias=1 fns=111 hidden=unset locals=kept comp=_after left=0"
    );
}

#[test]
fn without_a_prompt_deferred_code_runs_in_place() {
    let root = built();
    let run = |extra: &[(&str, &str)]| {
        let file = report(root.path());
        let _ = fs::remove_file(&file);
        let _ = fs::remove_file(root.path().join("order.log"));
        let shell = Shell::spawn(root.path(), &["-di", "-c", REPORT], extra);
        shell.finish();
        fs::read_to_string(file).unwrap()
    };
    let bundle = run(&[]);
    assert_eq!(bundle, run(&[("NO_BUNDLE", "1")]));
    assert_eq!(
        bundle.trim(),
        "order=compinit alpha beta main tool late locals after alias=1 fns=111 hidden=unset locals=kept comp=_after left=0"
    );
}

#[test]
fn a_file_with_top_level_declarations_stays_in_place() {
    let root = fixture();
    let built = build(root.path());
    assert!(
        built
            .stderr
            .contains("zsh/locals.zsh: declaration without -g; not deferred"),
        "{}",
        built.stderr
    );
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(bundle.contains("__zb_defer_4() {\nmark late"), "{bundle}");
    assert!(!bundle.contains("__zb_defer_5"), "{bundle}");
}

#[test]
fn keys_typed_before_the_prompt_run_after_the_deferred_code() {
    let root = built();
    let typed = root.path().join("typed");
    let mut shell = Shell::spawn(root.path(), &["-di"], &[("SLOW_START", "0.3")]);
    shell.type_keys(&format!(
        "cp $HOME/order.log {}\rprint -r -- second >| {}\r",
        typed.display(),
        root.path().join("second").display()
    ));
    assert_eq!(
        shell
            .wait_for_file(&typed)
            .split_whitespace()
            .collect::<Vec<_>>(),
        [
            "beta", "main", "locals", "after", "prompt", "compinit", "alpha", "tool", "late",
            "queued"
        ]
    );
    assert_eq!(
        shell.wait_for_file(&root.path().join("second")).trim(),
        "second"
    );
}

#[test]
fn keys_typed_while_deferred_code_runs_wait_for_it() {
    let root = built();
    let typed = root.path().join("typed");
    let mut shell = Shell::spawn(root.path(), &["-di"], &[("SLOW_COMPINIT", "0.6")]);
    shell.wait_until(prompt);
    std::thread::sleep(Duration::from_millis(150));
    shell.type_keys(&format!("cp $HOME/order.log {}\r", typed.display()));
    assert_eq!(
        shell
            .wait_for_file(&typed)
            .split_whitespace()
            .collect::<Vec<_>>(),
        [
            "beta", "main", "locals", "after", "prompt", "compinit", "alpha", "tool", "late",
            "queued"
        ]
    );
}

#[test]
fn keys_typed_before_the_prompt_see_deferred_key_bindings() {
    let root = built();
    let log = root.path().join("order.log");
    let mut shell = Shell::spawn(root.path(), &["-di"], &[("SLOW_START", "0.3")]);
    shell.type_keys("\x18a\r");
    shell.wait_until(|_| fs::read_to_string(&log).is_ok_and(|log| log.contains("widget")));
}

#[test]
fn line_init_hooks_added_after_deferral_run_on_the_first_prompt() {
    let root = built();
    let mut shell = Shell::spawn(root.path(), &["-di"], &[]);
    shell.wait_until(prompt);
    assert_eq!(shell.wait_for_file(&root.path().join("line-init")).trim(), "ok");
}
