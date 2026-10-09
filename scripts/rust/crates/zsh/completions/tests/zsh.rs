#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, executable, tree};

fn prepare(root: &Path, picker: &str) {
    let shim = Bin::new(env!("CARGO_BIN_EXE_zcomp"))
        .args(["--completions", "zsh"])
        .run();
    assert!(shim.success(), "{shim:?}");
    fs::write(root.join("completion.zsh"), shim.stdout).unwrap();
    executable(&root.join("bin/fzf"), picker);
}

fn pick(root: &Path, input: &str, query: &str) -> Ran {
    pick_selected(root, input, query, false)
}

fn pick_selected(root: &Path, input: &str, query: &str, selected: bool) -> Ran {
    let fzf = Bin::new("zsh")
        .args(["-dfc", "print -r -- $commands[fzf]"])
        .run();
    assert!(fzf.success() && !fzf.stdout.trim().is_empty(), "{fzf:?}");
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    Bin::new("zsh")
        .args([
            "-dfc",
            r#"
compdef() {}
defer() {}
source "$ZCOMP_TEST_ROOT/completion.zsh"
_zcomp_picked=$ZCOMP_TEST_SELECTED
_zcomp_fzf --ansi --delimiter='\x00' --nth=2,3 --no-sort --filter="$ZCOMP_TEST_QUERY"
"#,
        ])
        .env("ZCOMP_TEST_ROOT", root)
        .env("ZCOMP_TEST_QUERY", query)
        .env("ZCOMP_TEST_SELECTED", if selected { "1" } else { "0" })
        .env("ZCOMP_TEST_FZF", fzf.stdout.trim())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdin(input)
        .run()
}

#[test]
fn colored_selections_return_the_original_rows_for_fzf_tab_to_insert() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n\\n' \"$ZCOMP_TEST_QUERY\"\nexec \"$ZCOMP_TEST_FZF\" \"$@\"\n",
    );
    let editor = "\x1b[32m\0\x1b[36mcod-editor\x1b[0m  \x1b[34m116/mo\x1b[0m  editor [tools]\0\n";
    let scoped = "\x1b[33m\0\x1b[92m@\x1b[0m\x1b[34macme\x1b[0m/\x1b[36mcod-tools\x1b[0m  工具\0\n";
    let other = "\x1b[32m\0other  another package\0\n";
    let input = format!("{editor}{scoped}{other}");
    for (query, rows) in [
        ("cod-editor", editor.to_string()),
        ("cod", format!("{editor}{scoped}")),
    ] {
        let ran = pick(root.path(), &input, query);
        assert!(ran.success(), "{ran:?}");
        assert_eq!(ran.stdout, format!("{query}\n\n{rows}"));
    }
}

#[test]
fn plain_selections_and_the_query_and_expect_key_are_preserved() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n/\\n' \"$ZCOMP_TEST_QUERY\"\nexec \"$ZCOMP_TEST_FZF\" \"$@\"\n",
    );
    let row = "\0cod-editor  116/mo  editor\0\n";
    let ran = pick(root.path(), row, "cod-editor");
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.stdout, format!("cod-editor\n/\n{row}"));
}

#[test]
fn accepting_a_query_without_a_selection_preserves_the_query() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n' \"$ZCOMP_TEST_QUERY\"\n",
    );
    let ran = pick(root.path(), "\0cod-editor\0\n", "new-package");
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.stdout, "new-package\n");
}

#[test]
fn cancelling_the_picker_returns_no_selection_and_keeps_the_exit_status() {
    let root = tree(&["bin/"]);
    prepare(root.path(), "#!/bin/sh\nexit 130\n");
    let ran = pick(root.path(), "\0cod-editor\0\n", "cod");
    assert_eq!(ran.code(), Some(130), "{ran:?}");
    assert_eq!(ran.stdout, "");
}

#[test]
fn selected_packages_pass_through_fzf_tab_without_opening_another_picker() {
    let root = tree(&["bin/"]);
    prepare(root.path(), "#!/bin/sh\nexit 99\n");
    let rows = "\0shadcn\0\n\0@types/node\0\n";
    let ran = pick_selected(root.path(), rows, "", true);
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.stdout, format!("\n\n{rows}"));
}

// Exercise insertion through a real ZLE completion widget. The picker triggers its
// actual reload command deterministically, without timing network requests or keys.
fn complete_in_shell(query: &str, picker: &str) -> (String, String) {
    complete_word_in_shell("", query, picker)
}

fn complete_word_in_shell(prefix: &str, query: &str, picker: &str) -> (String, String) {
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use testkit::pty;

    let root = tree(&["bin/", "cache/"]);
    prepare(root.path(), picker);
    let hit = |name: &str, description: &str| {
        serde_json::json!({
            "name": name, "description": description, "version": "", "downloads": 1
        })
    };
    let values = [
        (
            "node-popular",
            serde_json::json!({"libraries": [hit("typescript", "shadcn in a description")], "tools": []}),
        ),
        (
            "search-registry.npmjs.org-shadcn",
            serde_json::json!([hit("shadcn", "registry result")]),
        ),
        (
            "versions-registry.npmjs.org-shadcn",
            serde_json::json!({"tags": [["latest", "3.0.0"]], "versions": ["3.0.0"]}),
        ),
    ];
    for (key, value) in values {
        let envelope = serde_json::json!({"stamp": "", "at": u64::MAX, "value": value});
        fs::write(
            root.path().join(format!("cache/{key}.json")),
            envelope.to_string(),
        )
        .unwrap();
    }
    // Help comes from fixtures, never a package manager on the test machine.
    fs::write(
        root.path().join("bun.txt"),
        include_str!("fixtures/bun.txt"),
    )
    .unwrap();
    fs::write(
        root.path().join("bun-add.txt"),
        include_str!("fixtures/bun-add.txt"),
    )
    .unwrap();
    executable(
        &root.path().join("bin/bun"),
        "#!/bin/sh\ncase $1 in add) cat \"$ZCOMP_TEST_ROOT/bun-add.txt\";; *) cat \"$ZCOMP_TEST_ROOT/bun.txt\";; esac\n",
    );
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_zcomp"), root.path().join("bin/zcomp")).unwrap();
    let startup = r#"
autoload -Uz compinit; compinit -D
defer() {}
source "$ZCOMP_TEST_ROOT/completion.zsh"
_test_complete() { local IN_FZF_TAB=1; _main_complete; }
zle -C test-complete complete-word _test_complete
bindkey '^I' test-complete
_report() { print -r -- "$BUFFER" > "$ZCOMP_TEST_ROOT/buffer"; }
zle -N report _report
bindkey '^Xr' report
PROMPT='READY> '
"#;
    fs::write(root.path().join(".zshrc"), startup).unwrap();
    let mut paths = vec![root.path().join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let (mut master, slave, _) = pty::open_pty(24, 100);
    let (stdin, stdout, stderr) = pty::stdio(&slave);
    let mut command = Command::new("zsh");
    command
        .arg("-di")
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("ZDOTDIR", root.path())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("TERM", "xterm-256color")
        .env("SHELL", "/bin/zsh")
        .env("ZCOMP_TEST_ROOT", root.path())
        .env("ZCOMP_TEST_QUERY", query)
        .env("ZCOMP_CACHE_DIR", root.path().join("cache"))
        .env("ZCOMP_OFFLINE", "1")
        .env("ZCOMP_FOREGROUND", "1")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    pty::take_controlling_terminal(&mut command);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    while Instant::now() < deadline && !String::from_utf8_lossy(&output).contains("READY> ") {
        pty::read_available(&master, &mut output, 20);
    }
    master
        .write_all(format!("bun i -d {prefix}\t\x18r").as_bytes())
        .unwrap();
    let buffer = root.path().join("buffer");
    while Instant::now() < deadline && !buffer.exists() {
        pty::read_available(&master, &mut output, 20);
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        !root.path().join("injected").exists(),
        "query was evaluated as shell code"
    );
    let inserted = fs::read_to_string(buffer).unwrap_or_else(|error| {
        panic!(
            "completion failed: {error}\n{}",
            String::from_utf8_lossy(&output)
        )
    });
    (
        inserted,
        fs::read_to_string(root.path().join("initial")).unwrap_or_default(),
    )
}

const RELOAD_PICKER: &str = r#"#!/bin/zsh -f
cat > "$ZCOMP_TEST_ROOT/initial"
for arg in "$@"; do
  if [[ $arg == --bind=change:* ]]; then
    reload=${arg#*reload\(}
    reload=${reload%\),load:*}
    reload=${reload//\{q\}/${(q)ZCOMP_TEST_QUERY}}
    eval "$reload" | head -n 1
  fi
done
"#;

#[test]
fn typing_in_the_package_picker_searches_beyond_the_initial_list() {
    let (buffer, initial) = complete_in_shell("shadcn", RELOAD_PICKER);
    assert!(initial.starts_with("typescript\t"), "{initial}");
    assert_eq!(buffer, "bun i -d shadcn \n");
}

#[test]
fn typing_a_version_in_the_picker_inserts_the_whole_spec() {
    let (buffer, _) = complete_in_shell("shadcn@lat", RELOAD_PICKER);
    assert_eq!(buffer, "bun i -d shadcn@latest \n");
}

#[test]
fn cancelling_package_search_keeps_the_original_command_line() {
    let (buffer, _) = complete_in_shell("", "#!/bin/sh\ncat >/dev/null\nexit 130\n");
    assert_eq!(buffer, "bun i -d \n");
}

#[test]
fn package_queries_are_passed_as_data_to_the_reload_command() {
    let (buffer, _) = complete_in_shell("$(touch injected)'", RELOAD_PICKER);
    assert_eq!(buffer, "bun i -d \n");
}

#[test]
fn multiple_package_selections_insert_specs_without_their_descriptions() {
    let (buffer, _) = complete_in_shell(
        "",
        "#!/bin/sh\ncat >/dev/null\nprintf 'shadcn@latest\\tCLI\\n@types/node\\tNode types\\n'\n",
    );
    assert_eq!(buffer, "bun i -d shadcn@latest @types/node \n");
}

#[test]
fn a_query_typed_before_tab_is_refreshed_when_the_picker_opens() {
    let picker = r#"#!/bin/zsh -f
cat > "$ZCOMP_TEST_ROOT/initial"
for arg in "$@"; do
  if [[ $arg == --bind=start:* ]]; then
    reload=${arg#*reload\(}
    reload=${reload%\)}
    reload=${reload//\{q\}/${(q)ZCOMP_TEST_QUERY}}
    eval "$reload" | head -n 1
  fi
done
"#;
    let (buffer, _) = complete_word_in_shell("shad", "shadcn", picker);
    assert_eq!(buffer, "bun i -d shadcn \n");
}
