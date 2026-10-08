#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, executable, tree_pairs};

const HELPERS: &str = r#"has_cmd() { (($+commands[$1])); }
add_path() {
  local -a valid_paths=()
  local dir
  for dir in "$@"; do
    [[ -d "$dir" ]] && valid_paths+=("$dir")
  done
  path=("${valid_paths[@]}" "${path[@]}")
}
cached_eval() {
  local cache="$DOTFILES_ZSH_CACHE/$1.zsh" bin
  shift
  bin=${commands[$1]:-$1}
  [[ -x $bin ]] || return 0

  if [[ ! -f $cache || $bin -nt $cache ]]; then
    mkdir -p "${cache:h}"
    "$@" >"$cache" 2>/dev/null || return 0
  fi

  source "$cache"
}
"#;

const PROBE: &str = "#!/bin/sh\necho probe >> \"$PROBE_LOG\"\nprintf 'folded value\\n'\n";

const TOOL: &str = "#!/bin/sh\necho tool >> \"$PROBE_LOG\"\nprintf 'local hidden=1\\ntool_completion() { print -r -- completed }\\ntool_zero=$0\\n'\n";

const REPORT: &str = r#"print -r -- "early=$early late=${late-unset} self=$self dir=$selfdir prompt=$prompt_file folded=$folded fn=$(tool_completion) hidden=${hidden-unset} zero=$tool_zero has=${has_probe-no} first=$path[1] f=$(f_zero) loop=$part""#;

fn fixture() -> tempfile::TempDir {
    let root = tree_pairs(&[
        (
            "config/zsh/build.toml",
            "output = \".cache/build\"\nambient = [\"HOME\"]\n\n[[target]]\nname = \"zshrc\"\nsource = \"zsh/rc.zsh\"\nenv = [\"zsh/env.zsh\"]\n\n[fold]\ncommands = [\"probe --value\", \"absent --value\"]\n",
        ),
        (
            "zsh/env.zsh",
            "export CONF=\"$HOME/zsh\"\nexport DOTFILES_ZSH_CACHE=\"$HOME/.cache/zsh\"\n",
        ),
        (
            "zsh/rc.zsh",
            "# zsh-build: omit\nif [[ -z $NO_BUNDLE && -r $HOME/.cache/build/zshrc.zsh ]]; then\n  source $HOME/.cache/build/zshrc.zsh\n  return\nfi\nsource \"$CONF/utils.zsh\"\nadd_path \"$HOME/bin\" \"$HOME/missing\"\nfor part in \"$CONF\"/parts/*.zsh(N); do\n  source \"$part\"\ndone\n",
        ),
        ("zsh/utils.zsh", HELPERS),
        (
            "zsh/parts/10-return.zsh",
            "early=1\n[[ -n $STOP ]] && return 0\nlate=1\n",
        ),
        (
            "zsh/parts/20-zero.zsh",
            "self=$0\nselfdir=${0:A:h}\nprompt_file=${(%):-%x}\nf_zero() { print -r -- $0 }\n",
        ),
        (
            "zsh/parts/30-fold.zsh",
            "folded=\"$(probe --value)\"\nhas_cmd probe && has_probe=yes\nhas_cmd absent && absent=\"$(absent --value)\"\n",
        ),
        (
            "zsh/parts/40-cached.zsh",
            "cached_eval tool-completion tool --completions zsh\n",
        ),
    ]);
    fs::create_dir_all(root.path().join("bin")).unwrap();
    executable(&root.path().join("bin/probe"), PROBE);
    executable(&root.path().join("bin/tool"), TOOL);
    root
}

fn path(root: &Path) -> String {
    format!("{}:/usr/bin:/bin", root.join("bin").display())
}

fn build(root: &Path, args: &[&str]) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("HOME", root)
        .env("PATH", path(root))
        .env("PROBE_LOG", root.join("build.log"))
        .run()
}

fn startup(root: &Path, bundle: bool, extra: &[(&str, &str)]) -> Ran {
    let mut command = Bin::new("zsh")
        .args([
            "-fc",
            &format!("source zsh/env.zsh; source zsh/rc.zsh; {REPORT}"),
        ])
        .current_dir(root)
        .env("HOME", root)
        .env("PATH", path(root))
        .env("PROBE_LOG", root.join("run.log"));
    if !bundle {
        command = command.env("NO_BUNDLE", "1");
    }
    for (name, value) in extra {
        command = command.env(name, value);
    }
    command.run()
}

fn log(root: &Path) -> String {
    fs::read_to_string(root.join("run.log")).unwrap_or_default()
}

#[test]
fn the_bundle_runs_like_the_sources() {
    let root = fixture();
    let built = build(root.path(), &[]);
    assert!(built.success(), "{built:?}");
    assert!(built.stderr.is_empty(), "{}", built.stderr);
    assert!(built.stdout.contains("updated"), "{}", built.stdout);

    let sources = startup(root.path(), false, &[]);
    let bundle = startup(root.path(), true, &[]);
    assert!(
        sources.success() && bundle.success(),
        "{sources:?} {bundle:?}"
    );
    assert_eq!(bundle.stdout, sources.stdout);
    assert!(sources.stdout.contains("late=1"), "{}", sources.stdout);
    assert!(
        sources.stdout.contains("folded=folded value"),
        "{}",
        sources.stdout
    );
    assert!(
        sources.stdout.contains("fn=completed hidden=unset"),
        "{}",
        sources.stdout
    );
    assert!(sources.stdout.contains("f=f_zero"), "{}", sources.stdout);
}

#[test]
fn a_return_still_leaves_only_its_file() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    let sources = startup(root.path(), false, &[("STOP", "1")]);
    let bundle = startup(root.path(), true, &[("STOP", "1")]);
    assert_eq!(bundle.stdout, sources.stdout);
    assert!(
        bundle.stdout.contains("early=1 late=unset"),
        "{}",
        bundle.stdout
    );
    assert!(
        bundle.stdout.contains("folded=folded value"),
        "{}",
        bundle.stdout
    );
}

#[test]
fn folded_commands_do_not_run_at_startup() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    startup(root.path(), false, &[]);
    assert_eq!(log(root.path()), "probe\ntool\n");
    fs::remove_file(root.path().join("run.log")).unwrap();
    startup(root.path(), true, &[]);
    assert_eq!(log(root.path()), "");
}

#[test]
fn rebuilding_unchanged_sources_rewrites_nothing() {
    let root = fixture();
    let dry = build(root.path(), &["--dry-run"]);
    assert!(dry.stdout.contains("would update"), "{}", dry.stdout);
    assert!(!root.path().join(".cache/build/zshrc.zsh").exists());
    assert!(build(root.path(), &[]).success());
    let again = build(root.path(), &[]);
    assert!(again.stdout.contains("current"), "{}", again.stdout);
}

#[test]
fn an_unrecognized_helper_is_left_to_run() {
    let root = fixture();
    let utils = root.path().join("zsh/utils.zsh");
    let changed = fs::read_to_string(&utils)
        .unwrap()
        .replace("valid_paths+=(\"$dir\")", "valid_paths+=($dir)");
    fs::write(&utils, changed).unwrap();
    let built = build(root.path(), &[]);
    assert!(
        built.stderr.contains("add_path: unrecognized definition"),
        "{}",
        built.stderr
    );
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(bundle.contains("add_path \"$HOME/bin\""), "{bundle}");
    assert_eq!(
        startup(root.path(), true, &[]).stdout,
        startup(root.path(), false, &[]).stdout
    );
}

#[test]
fn the_wordcode_is_compiled_in_the_background_and_used() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let wordcode = root.path().join(".cache/build/zshrc.zsh.zwc");
    startup(root.path(), true, &[]);
    for _ in 0..50 {
        if wordcode.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(wordcode.exists());
    assert!(
        fs::metadata(&wordcode).unwrap().modified().unwrap()
            >= fs::metadata(&bundle).unwrap().modified().unwrap()
    );
    assert_eq!(
        startup(root.path(), true, &[]).stdout,
        startup(root.path(), false, &[]).stdout
    );
}

#[test]
fn only_the_dotfiles_own_code_warns_when_left_to_run() {
    let root = fixture();
    let outside = tempfile::tempdir().unwrap();
    let unrewritable = "( return 1 )\n";
    fs::write(outside.path().join("vendor.zsh"), unrewritable).unwrap();
    fs::write(root.path().join("zsh/parts/50-own.zsh"), unrewritable).unwrap();
    let rc = format!(
        "source {}\nsource {}\n",
        root.path().join("zsh/parts/50-own.zsh").display(),
        outside.path().join("vendor.zsh").display()
    );
    fs::write(root.path().join("zsh/rc.zsh"), rc).unwrap();

    let built = zsh_build::build(&zsh_build::Options {
        root: root.path().to_path_buf(),
        dry_run: true,
    })
    .unwrap()
    .remove(0);
    assert!(
        built
            .warnings
            .iter()
            .any(|note| note.contains("50-own.zsh")),
        "{built:?}"
    );
    assert!(
        built.skipped.iter().any(|note| note.contains("vendor.zsh")),
        "{built:?}"
    );
    assert!(
        !built
            .warnings
            .iter()
            .any(|note| note.contains("vendor.zsh")),
        "{built:?}"
    );
}

#[test]
fn wordcode_compiled_from_an_older_bundle_is_discarded() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let wordcode = root.path().join(".cache/build/zshrc.zsh.zwc");
    let compile_line = |text: &str| {
        text.lines()
            .nth(1)
            .and_then(|line| line.strip_suffix(" &!"))
            .unwrap()
            .to_string()
    };
    let older = compile_line(&fs::read_to_string(&bundle).unwrap());
    fs::write(root.path().join("zsh/parts/60-new.zsh"), "added=1\n").unwrap();
    assert!(build(root.path(), &[]).success());
    let current = compile_line(&fs::read_to_string(&bundle).unwrap());
    assert_ne!(older, current);

    assert!(Bin::new("zsh").args(["-fc", &older]).run().code() != Some(127));
    assert!(!wordcode.exists());
    assert!(Bin::new("zsh").args(["-fc", &current]).run().success());
    assert!(wordcode.exists());
}

#[test]
fn bundled_functions_report_where_they_were_defined() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    let zero = root.path().join("zsh/parts/20-zero.zsh");
    let located = build(
        root.path(),
        &["where", "f_zero", "tool_completion", "missing"],
    );
    assert!(!located.success());
    assert!(
        located
            .stdout
            .contains(&format!("f_zero  {}:4", zero.display())),
        "{}",
        located.stdout
    );
    assert!(
        located.stdout.contains(".cache/zsh/tool-completion.zsh:2"),
        "{}",
        located.stdout
    );
    assert!(
        located.stderr.contains("missing: not a bundled function"),
        "{}",
        located.stderr
    );

    let lookup = "type f_zero; whence -v f_zero; whence -w f_zero; f() { type -a f_zero }; f; type nosuch; print status=$?";
    let run = |bundle: bool| {
        let mut command = Bin::new("zsh")
            .args([
                "-fc",
                &format!("source zsh/env.zsh; source zsh/rc.zsh; {lookup}"),
            ])
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("PATH", path(root.path()))
            .env("PROBE_LOG", root.path().join("run.log"));
        if !bundle {
            command = command.env("NO_BUNDLE", "1");
        }
        command.run().stdout
    };
    let sources = run(false);
    let bundle = run(true);
    assert_eq!(
        bundle,
        sources.replace(
            &format!("from {}", zero.display()),
            &format!("from {}:4", zero.display())
        )
    );
    assert!(bundle.contains("f_zero: function"), "{bundle}");
    assert!(bundle.ends_with("nosuch not found\nstatus=1\n"), "{bundle}");
}
