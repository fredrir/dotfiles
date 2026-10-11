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
    shell(root, bundle, REPORT, extra)
}

/// `script` after the startup files, from the bundle or the sources.
fn shell(root: &Path, bundle: bool, script: &str, extra: &[(&str, &str)]) -> Ran {
    let mut command = Bin::new("zsh")
        .args([
            "-fc",
            &format!("source zsh/env.zsh; source zsh/rc.zsh; {script}"),
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
fn an_error_still_stops_only_its_file() {
    let root = fixture();
    fs::write(
        root.path().join("zsh/parts/15-error.zsh"),
        "erred=1\n: $(( 1 / 0 ))\nunreached=1\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success(), "{built:?}");
    let report = r#"print -r -- "erred=$erred unreached=${unreached-unset} folded=$folded fn=$(tool_completion)""#;
    let sources = shell(root.path(), false, report, &[]);
    let bundle = shell(root.path(), true, report, &[]);
    assert_eq!(bundle.stdout, sources.stdout);
    assert!(
        bundle
            .stdout
            .contains("erred=1 unreached=unset folded=folded value fn=completed"),
        "{}",
        bundle.stdout
    );
}

#[test]
fn the_status_after_a_file_is_what_source_gives() {
    let root = fixture();
    let files = [
        (
            "zsh/inner-error.zsh",
            "inner_zero=${0:t}\n: $(( 1 / 0 ))\ninner_late=1\n",
        ),
        ("zsh/inner-ok.zsh", "inner_ok=1\nfalse\n"),
        (
            "zsh/parts/15-status.zsh",
            "source \"$CONF/inner-error.zsh\"\nerror_status=$?\nouter_zero=${0:t}\nsource \"$CONF/inner-ok.zsh\"\nok_status=$?\n",
        ),
    ];
    for (path, text) in files {
        fs::write(root.path().join(path), text).unwrap();
    }
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(
        bundle.contains("inner_late=1") && bundle.contains("_zsh_build_status\n"),
        "{bundle}"
    );
    let report = same(
        root.path(),
        r#"print -r -- "error=$error_status ok=$ok_status zero=$inner_zero,$outer_zero late=${inner_late-unset} stopped=${_zsh_build_stopped-unset}""#,
    );
    assert_eq!(
        report,
        "error=126 ok=1 zero=inner-error.zsh,15-status.zsh late=unset stopped=unset\n"
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

fn wordcode_version(path: &Path) -> String {
    let header = fs::read(path).unwrap();
    let version = &header[8..];
    String::from_utf8(version[..version.iter().position(|byte| *byte == 0).unwrap()].to_vec())
        .unwrap()
}

fn shell_version() -> String {
    Bin::new("zsh")
        .args(["-fc", "print -r -- $ZSH_VERSION"])
        .run()
        .stdout
        .trim()
        .to_string()
}

fn modified(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn the_build_compiles_the_wordcode_the_bundle_uses() {
    let root = fixture();
    let bundle = root.path().join(".cache/build/zshrc.zsh");
    let wordcode = root.path().join(".cache/build/zshrc.zsh.zwc");
    assert!(build(root.path(), &["--dry-run"]).success());
    assert!(!wordcode.exists());
    assert!(build(root.path(), &[]).success());
    assert!(modified(&wordcode) > modified(&bundle));
    assert_eq!(wordcode_version(&wordcode), shell_version());
    let compiled = fs::read(&wordcode).unwrap();
    let staging = root.path().join("prelude.zwc");
    assert!(
        Bin::new("zsh")
            .args(["-fc", "zcompile -U $1 $2", "zsh"])
            .arg(&staging)
            .arg(&bundle)
            .run()
            .success()
    );
    assert_eq!(compiled, fs::read(&staging).unwrap());

    let again = build(root.path(), &[]);
    assert!(again.stdout.contains("current"), "{}", again.stdout);
    fs::remove_file(&wordcode).unwrap();
    let dry = build(root.path(), &["--dry-run"]);
    assert!(dry.stdout.contains("would update"), "{}", dry.stdout);
    assert!(!wordcode.exists());
    let rebuilt = build(root.path(), &[]);
    assert!(rebuilt.stdout.contains("updated"), "{}", rebuilt.stdout);
    assert_eq!(fs::read(&wordcode).unwrap(), compiled);

    startup(root.path(), true, &[]);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(fs::read(&wordcode).unwrap(), compiled);
    assert_eq!(
        startup(root.path(), true, &[]).stdout,
        startup(root.path(), false, &[]).stdout
    );
}

#[test]
fn wordcode_from_another_zsh_version_is_recompiled_by_the_shell() {
    let root = fixture();
    assert!(build(root.path(), &[]).success());
    let wordcode = root.path().join(".cache/build/zshrc.zsh.zwc");
    let mut header = fs::read(&wordcode).unwrap();
    header[8..11].copy_from_slice(b"0.0");
    let stale = root.path().join("stale.zwc");
    fs::write(&stale, &header).unwrap();
    fs::rename(&stale, &wordcode).unwrap();
    assert_eq!(wordcode_version(&wordcode), "0.0");

    startup(root.path(), true, &[]);
    for _ in 0..50 {
        if wordcode_version(&wordcode) != "0.0" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(wordcode_version(&wordcode), shell_version());
    let rebuilt = build(root.path(), &[]);
    assert!(rebuilt.stdout.contains("current"), "{}", rebuilt.stdout);
}

#[test]
fn without_zsh_the_build_warns_and_leaves_the_wordcode_to_the_shell() {
    let root = fixture();
    let built = Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root.path())
        .env("HOME", root.path())
        .env("PATH", root.path().join("bin"))
        .env_remove("SHELL")
        .env("PROBE_LOG", root.path().join("build.log"))
        .run();
    assert!(built.success(), "{built:?}");
    assert!(
        built
            .stderr
            .contains("zsh not found; wordcode left to the shell"),
        "{}",
        built.stderr
    );
    assert!(!root.path().join(".cache/build/zshrc.zsh.zwc").exists());
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
    fs::remove_file(&wordcode).unwrap();

    assert!(Bin::new("zsh").args(["-fc", &older]).run().code() != Some(127));
    assert!(!wordcode.exists());
    assert!(Bin::new("zsh").args(["-fc", &current]).run().success());
    assert!(wordcode.exists());
}

#[test]
fn a_function_defined_after_an_alias_of_its_name_fails_the_build() {
    let root = fixture();
    let part = root.path().join("zsh/parts/50-alias.zsh");
    fs::write(
        &part,
        "has_cmd probe && alias cat=probe\nfunction cat { :; }\n[[ $UNDECIDED == x ]] && alias ls=probe\nls() { :; }\nif has_cmd probe; then\n  cat() { :; }\nfi\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(!built.success());
    assert_eq!(
        built.stderr.trim_end(),
        format!(
            "zsh-build: {}:6: cat: alias redefined as function",
            part.display()
        )
    );
}

#[test]
fn a_function_may_take_the_name_of_an_unaliased_or_suffix_alias() {
    let root = fixture();
    fs::write(
        root.path().join("zsh/parts/50-alias.zsh"),
        "alias cat=probe\nunalias cat\ncat() { :; }\nalias -s txt=probe\ntxt() { :; }\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success(), "{}", built.stderr);
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

/// Prints the form `atuin uuid` prints, made now, with a fixed random part.
const ATUIN: &str = "#!/bin/sh\necho atuin >> \"$PROBE_LOG\"\nprintf '%012x712389abcdef01234567\\n' $(( $(date +%s) * 1000 ))\n";

const SESSIONS: &str =
    "export ATUIN_SESSION=$(atuin uuid)\nsessions() {\n  repeat $1 ids+=(\"$(atuin uuid)\")\n}\n";

fn session_fixture(atuin: Option<&str>) -> tempfile::TempDir {
    let root = fixture();
    fs::write(root.path().join("zsh/parts/50-session.zsh"), SESSIONS).unwrap();
    if let Some(atuin) = atuin {
        executable(&root.path().join("bin/atuin"), atuin);
    }
    root
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[test]
fn atuin_session_ids_are_made_natively_like_atuin_uuid() {
    let root = session_fixture(Some(ATUIN));
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
    assert!(
        bundle.contains(
            "ATUIN_SESSION=${${:-$((_zsh_build_uuid7()))}:+${_zsh_build_uuid:-$(atuin uuid)}}"
        ),
        "{bundle}"
    );

    let before = unix_millis();
    let ran = shell(
        root.path(),
        true,
        "sessions 500; print -rl -- $ATUIN_SESSION $ids",
        &[],
    );
    let after = unix_millis();
    assert!(ran.success() && ran.stderr.is_empty(), "{ran:?}");
    assert_eq!(log(root.path()), "");
    let ids: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(ids.len(), 501);
    for id in &ids {
        let form = id.len() == 32
            && id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            && id.as_bytes()[12] == b'7'
            && id.as_bytes()[13] <= b'7'
            && matches!(id.as_bytes()[16], b'8' | b'9' | b'a' | b'b');
        assert!(form, "{id}");
    }
    let values: Vec<u128> = ids
        .iter()
        .map(|id| u128::from_str_radix(id, 16).unwrap())
        .collect();
    let millis: Vec<u128> = values.iter().map(|value| value >> 80).collect();
    assert!(
        millis.windows(2).all(|pair| pair[0] <= pair[1]),
        "{millis:?}"
    );
    assert!(
        before <= millis[0] && millis[500] <= after,
        "{before} {millis:?} {after}"
    );
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        501
    );

    let low = (1u128 << 80) - 1;
    let fixed_ones = (0b0111 << 76) | (1 << 63);
    let fixed_zeros = (1 << 79) | (1 << 75) | (1 << 62);
    let all = values.iter().fold(low, |all, value| all & value);
    let any = values.iter().fold(0, |any, value| any | (value & low));
    assert_eq!(all, fixed_ones, "{all:#x}");
    assert_eq!(any, low & !fixed_zeros, "{any:#x}");
}

#[test]
fn atuin_uuid_still_runs_from_the_sources_or_when_the_native_code_fails() {
    let root = session_fixture(Some(ATUIN));
    assert!(build(root.path(), &[]).success());
    let sources = shell(root.path(), false, "sessions 1; print -rl -- $ids", &[]);
    assert!(
        sources.stdout.ends_with("712389abcdef01234567\n"),
        "{sources:?}"
    );
    assert_eq!(log(root.path()), "probe\ntool\natuin\natuin\n");

    fs::remove_file(root.path().join("run.log")).unwrap();
    let failing = shell(
        root.path(),
        true,
        "sysread() { return 5 }; sessions 2; print -rl -- $ATUIN_SESSION $ids",
        &[],
    );
    let ids: Vec<&str> = failing.stdout.lines().collect();
    assert_eq!(ids.len(), 3, "{failing:?}");
    assert!(!ids[0].ends_with("712389abcdef01234567"), "{ids:?}");
    assert!(
        ids[1..]
            .iter()
            .all(|id| id.ends_with("712389abcdef01234567")),
        "{ids:?}"
    );
    assert_eq!(log(root.path()), "atuin\natuin\n");
}

#[test]
fn atuin_uuid_is_left_to_run_when_it_prints_another_form_or_is_missing() {
    let hyphenated = "#!/bin/sh\necho 01a120c9-8fed-74a3-beea-dec383fcc3b8\n";
    for atuin in [Some(hyphenated), None] {
        let root = session_fixture(atuin);
        let built = build(root.path(), &[]);
        assert!(built.success(), "{built:?}");
        assert_eq!(
            built
                .stderr
                .contains("atuin uuid: unexpected output; left to runtime"),
            atuin.is_some(),
            "{}",
            built.stderr
        );
        let bundle = fs::read_to_string(root.path().join(".cache/build/zshrc.zsh")).unwrap();
        assert!(
            bundle.contains("export ATUIN_SESSION=$(atuin uuid)\n"),
            "{bundle}"
        );
        assert!(!bundle.contains("_zsh_build_uuid"), "{bundle}");
    }
}

fn build_log(root: &Path) -> String {
    fs::read_to_string(root.join("build.log")).unwrap_or_default()
}

#[test]
fn command_output_is_kept_until_the_program_or_its_checkout_changes() {
    let root = session_fixture(Some(ATUIN));
    let log = root.path().join("build.log");
    let head = root.path().join(".git/HEAD");
    fs::create_dir_all(head.parent().unwrap()).unwrap();
    fs::write(&head, "abc\n").unwrap();
    assert!(build(root.path(), &["--dry-run"]).success());
    assert!(!root.path().join(".cache/build/commands.json").exists());
    fs::remove_file(&log).unwrap();
    assert!(build(root.path(), &[]).success());
    assert_eq!(build_log(root.path()), "probe\ntool\natuin\n");

    fs::remove_file(&log).unwrap();
    let again = build(root.path(), &[]);
    assert!(again.stdout.contains("current"), "{}", again.stdout);
    assert_eq!(build_log(root.path()), "atuin\n");

    fs::remove_file(&log).unwrap();
    executable(
        &root.path().join("bin/tool"),
        &TOOL.replace("tool_zero", "tool_zero2"),
    );
    let changed = build(root.path(), &[]);
    assert!(changed.stdout.contains("updated"), "{}", changed.stdout);
    assert_eq!(build_log(root.path()), "tool\natuin\n");

    fs::remove_file(&log).unwrap();
    fs::write(&head, "def\n").unwrap();
    assert!(build(root.path(), &[]).success());
    assert_eq!(build_log(root.path()), "probe\ntool\natuin\n");
}

fn bundle_text(root: &Path) -> String {
    fs::read_to_string(root.join(".cache/build/zshrc.zsh")).unwrap()
}

/// `report` after startup from the bundle and from the sources, which must match.
fn same(root: &Path, report: &str) -> String {
    let sources = shell(root, false, report, &[]);
    let bundle = shell(root, true, report, &[]);
    assert!(
        sources.success() && bundle.success(),
        "{sources:?} {bundle:?}"
    );
    assert_eq!(bundle.stdout, sources.stdout);
    bundle.stdout
}

const COMMANDS: &str = r#"if has_cmd absent; then
  arm=absent
elif (( $+commands[probe] )); then
  arm=probe
else
  arm=none
fi
has_cmd absent && skipped=1
skipped_status=$?
(( ! ${+commands[probe]} )) ||
  kept=1
if has_cmd absent || has_cmd probe; then
  either=1
fi
if (( $+commands[absent] )); then
  dropped=1
fi
[[ -z $unset_var ]] && by_var=1
"#;

#[test]
fn command_tests_decided_at_build_time_leave_only_the_code_that_runs() {
    let root = fixture();
    fs::write(root.path().join("zsh/parts/50-commands.zsh"), COMMANDS).unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    for gone in [
        "has_cmd absent",
        "commands[probe]",
        "commands[absent]",
        "arm=absent",
        "arm=none",
        "skipped=1",
        "dropped=1",
    ] {
        assert!(!bundle.contains(gone), "{gone}: {bundle}");
    }
    for kept in [
        "  arm=probe\n",
        "! :\nskipped_status=$?\n",
        "\nkept=1\n",
        "  either=1\n",
        "[[ -z $unset_var ]] && by_var=1\n",
    ] {
        assert!(bundle.contains(kept), "{kept}: {bundle}");
    }
    let report = same(
        root.path(),
        r#"print -r -- "arm=$arm skipped=${skipped-unset}:$skipped_status kept=$kept either=$either dropped=${dropped-unset} by_var=$by_var""#,
    );
    assert!(
        report.contains("arm=probe skipped=unset:1 kept=1 either=1 dropped=unset by_var=1"),
        "{report}"
    );
}

const LOOKUPS: &str = "editor=\"$(command -v probe)\"\nmode=$(emulate)\n";

#[test]
fn command_paths_and_the_emulation_are_known_at_build_time() {
    let root = fixture();
    let part = root.path().join("zsh/parts/50-lookups.zsh");
    let builtin = "printer=\"$(command -v printf)\"\n";
    fs::write(&part, format!("{LOOKUPS}{builtin}")).unwrap();
    assert!(build(root.path(), &[]).success());
    let probe = root.path().join("bin/probe");
    let expected = format!("editor=\"{}\"\nmode=zsh\n{builtin}", probe.display());
    assert!(bundle_text(root.path()).contains(&expected), "{expected}");
    let report = same(
        root.path(),
        r#"print -r -- "editor=$editor mode=$mode printer=$printer""#,
    );
    assert!(
        report.contains(&format!(
            "editor={} mode=zsh printer=printf",
            probe.display()
        )),
        "{report}"
    );
}

#[test]
fn command_paths_and_the_emulation_stay_lookups_once_code_may_change_them() {
    let root = fixture();
    fs::write(
        root.path().join("zsh/parts/50-lookups.zsh"),
        format!("probe() {{ :; }}\nf() {{ emulate sh; }}\n{LOOKUPS}"),
    )
    .unwrap();
    assert!(build(root.path(), &[]).success());
    assert!(bundle_text(root.path()).contains(LOOKUPS));
    let report = same(root.path(), r#"print -r -- "editor=$editor mode=$mode""#);
    assert!(report.contains("editor=probe mode=zsh"), "{report}");
}

const INIT: &str = "#!/bin/sh\necho init >> \"$PROBE_LOG\"\ncase $1 in\n  --zsh) printf 'init_fn() { print -r -- initialized }\\ninit_ready=1\\n' ;;\n  --frames) printf 'init_frames=${#funcstack}\\n' ;;\nesac\n";

const SETUP: &str = "setup() {\n  local ready=1\n  eval \"$(init --zsh)\"\n}\nframes() {\n  eval \"$(init --frames)\"\n}\nsetup\nframes\n";

#[test]
fn folded_code_a_function_evaluates_is_parsed_with_the_bundle() {
    let root = fixture();
    let config = root.path().join("config/zsh/build.toml");
    let text = fs::read_to_string(&config).unwrap().replace(
        "\"absent --value\"",
        "\"absent --value\", \"init --zsh\", \"init --frames\"",
    );
    fs::write(&config, text).unwrap();
    executable(&root.path().join("bin/init"), INIT);
    let part = root.path().join("zsh/parts/50-setup.zsh");
    fs::write(&part, SETUP).unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(
        bundle.contains(
            "  {\ninit_fn() { print -r -- initialized }\ninit_ready=1\n} always { TRY_BLOCK_ERROR=0 }\n}\n"
        ),
        "{bundle}"
    );
    assert!(!bundle.contains("$(init"), "{bundle}");
    assert_eq!(bundle.matches("eval \"${__zb").count(), 1, "{bundle}");

    let report = same(
        root.path(),
        r#"print -r -- "fn=$(init_fn) ready=$init_ready frames=$init_frames""#,
    );
    assert!(
        report.starts_with("fn=initialized ready=1 frames="),
        "{report}"
    );
    fs::remove_file(root.path().join("run.log")).unwrap();
    shell(root.path(), true, ":", &[]);
    assert_eq!(log(root.path()), "");

    let located = build(root.path(), &["where", "init_fn"]);
    assert!(
        located
            .stdout
            .contains(&format!("init_fn  {}:3", part.display())),
        "{}",
        located.stdout
    );
}

#[test]
fn what_a_condition_reads_is_unknown_after_it() {
    let root = fixture();
    fs::create_dir_all(root.path().join("zsh/opt")).unwrap();
    fs::write(root.path().join("zsh/opt/real.zsh"), "picked=real\n").unwrap();
    fs::write(root.path().join("zsh/opt/missing.zsh"), "picked=missing\n").unwrap();
    fs::write(root.path().join("choice"), "real\n").unwrap();
    fs::write(
        root.path().join("zsh/parts/50-choice.zsh"),
        "choice=missing\nif ! read -r choice < \"$HOME/choice\"; then\n  :\nfi\nsource \"$CONF/opt/$choice.zsh\"\n",
    )
    .unwrap();
    assert!(build(root.path(), &[]).success());
    let report = same(root.path(), r#"print -r -- "picked=$picked""#);
    assert!(report.contains("picked=real"), "{report}");
}

#[test]
fn dropped_code_takes_its_returns_and_file_references_with_it() {
    let root = fixture();
    fs::write(
        root.path().join("zsh/parts/50-drop.zsh"),
        "(( $+commands[probe] )) || return\nif has_cmd absent; then\n  dropped_file=${(%):-%x}\n  return 1\nfi\nafter_drop=1\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(bundle.contains(":\n:\nafter_drop=1\n"), "{bundle}");
    assert!(!bundle.contains("dropped_file"), "{bundle}");
    let report = same(root.path(), r#"print -r -- "after=$after_drop""#);
    assert!(report.contains("after=1"), "{report}");
}

#[test]
fn a_return_inside_a_loop_still_leaves_only_its_file() {
    let root = fixture();
    fs::write(
        root.path().join("zsh/parts/50-loop.zsh"),
        "for word in a b; do\n  [[ $word == a ]] && return\n  looped=$word\ndone\nafter_loop=1\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(bundle.contains("[[ $word == a ]] && break 2\n"), "{bundle}");
    let report = same(
        root.path(),
        r#"print -r -- "looped=${looped-unset} after=${after_loop-unset}""#,
    );
    assert!(report.contains("looped=unset after=unset"), "{report}");
}

const ONCE: &str = "setup_probe() {\n  (( ${+commands[probe]} )) || return 1\n  has_cmd absent && return 1\n  setup_by=probe\n}\nsetup_fallback() {\n  (( $+commands[probe] )) || return 1\n  setup_by=fallback\n}\nsetup_probe || setup_fallback\nunset -f -m 'setup_*'\n() {\n  has_cmd absent && return\n  anonymous=1\n}\n";

#[test]
fn command_tests_in_a_function_run_once_at_startup_are_decided() {
    let root = fixture();
    fs::write(root.path().join("zsh/parts/50-once.zsh"), ONCE).unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(!bundle.contains("${+commands[probe]}"), "{bundle}");
    assert!(
        bundle.contains("  (( $+commands[probe] )) || return 1\n  setup_by=fallback\n"),
        "{bundle}"
    );
    let report = same(
        root.path(),
        r#"print -r -- "by=$setup_by left=${(k)functions[(I)setup_*]} anonymous=$anonymous""#,
    );
    assert!(report.contains("by=probe left= anonymous=1"), "{report}");
}

const SUBSHELL: &str = "#!/bin/sh\necho subshell >> \"$PROBE_LOG\"\nprintf 'sub_fn() {\\n  if ((( $# )) && [[ $1 == x ]]); then print -r -- matched; else print -r -- other; fi\\n}\\n'\n";

#[test]
fn folded_code_zsh_reads_as_a_subshell_is_respelled_to_parse() {
    let root = fixture();
    let config = root.path().join("config/zsh/build.toml");
    let text = fs::read_to_string(&config).unwrap().replace(
        "\"absent --value\"",
        "\"absent --value\", \"subshell --zsh\"",
    );
    fs::write(&config, text).unwrap();
    executable(&root.path().join("bin/subshell"), SUBSHELL);
    fs::write(
        root.path().join("zsh/parts/50-subshell.zsh"),
        "load() {\n  eval \"$(subshell --zsh)\"\n}\nload\n",
    )
    .unwrap();
    let built = build(root.path(), &[]);
    assert!(built.success() && built.stderr.is_empty(), "{built:?}");
    let bundle = bundle_text(root.path());
    assert!(
        bundle.contains("if ( (( $# )) && [[ $1 == x ]]); then"),
        "{bundle}"
    );
    assert!(!bundle.contains("$(subshell"), "{bundle}");
    let report = same(root.path(), r#"print -r -- "$(sub_fn x) $(sub_fn)""#);
    assert!(report.contains("matched other"), "{report}");
    fs::remove_file(root.path().join("run.log")).unwrap();
    shell(root.path(), true, ":", &[]);
    assert_eq!(log(root.path()), "");
}
