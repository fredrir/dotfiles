#![forbid(unsafe_code)]

use std::{fs, path::Path};
use testkit::{Bin, Ran, tree_pairs};

fn run(root: &Path, args: &[&str], input: &str) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_luafmt"))
        .args(args)
        .current_dir(root)
        .plain()
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .stdin(input)
        .run()
}

#[test]
fn stdin_editor_and_check_match_the_formatter_cli_contract() {
    let root = tree_pairs(&[("untouched.lua", "local x=2")]);
    for args in [
        vec![],
        vec!["-"],
        vec!["--stdin", "a.lua"],
        vec!["-eq"],
        vec!["-ev"],
        vec!["--editor", "--stdin", "a.lua"],
    ] {
        let output = run(root.path(), &args, "local x=1");
        assert_eq!(output.code(), Some(0), "{}", output.stderr);
        assert_eq!(output.stdout, "local x = 1");
        assert_eq!(output.stderr, "");
    }
    for args in [
        vec!["--check"],
        vec!["--check", "-"],
        vec!["-eq", "--check"],
    ] {
        let output = run(root.path(), &args, "local x=1");
        assert_eq!(output.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(run(root.path(), &args, "local x = 1").code(), Some(0));
    }
    assert_eq!(
        fs::read_to_string(root.path().join("untouched.lua")).unwrap(),
        "local x=2"
    );
    assert_eq!(run(root.path(), &["-e", "a.lua"], "").code(), Some(1));
    assert_eq!(
        run(root.path(), &["--stdin", "a.lua", "a.lua"], "").code(),
        Some(2)
    );
}

#[test]
fn walks_lua_files_reports_in_order_and_skips_build_trees() {
    let root = tree_pairs(&[
        ("z.lua", "local z=1"),
        ("nested/a.LUA", "local a=1"),
        ("typed.luau", "local n: number=1"),
        ("skip.txt", "not lua"),
        ("node_modules/skip.lua", "not lua"),
        ("target/skip.lua", "not lua"),
    ]);
    let check = run(root.path(), &["--check", "."], "");
    assert_eq!(check.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert!(
        check.stderr.contains("3 of 3 files need formatting"),
        "{}",
        check.stderr
    );
    assert_eq!(
        fs::read_to_string(root.path().join("z.lua")).unwrap(),
        "local z=1"
    );
    let output = run(root.path(), &["."], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert!(output.stderr.find("nested/a.LUA").unwrap() < output.stderr.find("z.lua").unwrap());
    assert_eq!(run(root.path(), &["--check", "."], "").code(), Some(0));
    assert_eq!(run(root.path(), &["-q", "."], "").stderr, "");
    assert_eq!(
        fs::read_to_string(root.path().join("target/skip.lua")).unwrap(),
        "not lua"
    );
}

#[test]
fn explicit_files_and_stdin_can_be_combined() {
    let root = tree_pairs(&[("script", "local x=1")]);
    let output = run(root.path(), &["-", "script"], "local y=2");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "local y = 2");
    assert_eq!(
        fs::read_to_string(root.path().join("script")).unwrap(),
        "local x = 1"
    );
}

#[test]
fn overlapping_directories_and_explicit_files_are_formatted_once() {
    let root = tree_pairs(&[("a.lua", "local a=1"), ("sub/b.lua", "local b=2")]);
    let output = run(root.path(), &["a.lua", "sub/b.lua", ".", "sub"], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert!(
        output.stderr.contains("formatted 2 of 2 files"),
        "{}",
        output.stderr
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local a = 1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("sub/b.lua")).unwrap(),
        "local b = 2"
    );
}

#[test]
fn invalid_stdin_does_not_prevent_other_targets_from_formatting() {
    let root = tree_pairs(&[("a.lua", "local a=1"), ("b.lua", "local b=2")]);
    let output = run(root.path(), &["a.lua", "-", "b.lua"], "local =");
    assert_eq!(output.code(), Some(1));
    assert!(
        output.stderr.contains("stdin: error parsing"),
        "{}",
        output.stderr
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local a = 1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local b = 2"
    );
}

#[test]
fn long_valid_filenames_can_be_replaced_without_leaving_temporary_files() {
    let root = tree_pairs(&[]);
    let name = format!("{}.lua", "a".repeat(246));
    fs::write(root.path().join(&name), "local x=1").unwrap();
    let output = run(root.path(), &[&name], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(
        fs::read_to_string(root.path().join(&name)).unwrap(),
        "local x = 1"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn special_files_are_rejected_without_waiting_for_a_writer() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let root = tree_pairs(&[]);
    let fifo = root.path().join("pipe.lua");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Bin::new(env!("CARGO_BIN_EXE_luafmt"))
        .arg(&fifo)
        .current_dir(root.path())
        .plain()
        .stdio(Stdio::null(), Stdio::piped(), Stdio::piped())
        .spawn();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("formatter blocked reading a FIFO");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a regular file"));
    let output = run(root.path(), &["/dev/null"], "");
    assert_eq!(output.code(), Some(1));
    assert!(output.stderr.contains("not a regular file"));
}

#[cfg(unix)]
#[test]
fn quiet_mode_still_reports_unreadable_directories() {
    use std::os::unix::fs::PermissionsExt;
    let root = tree_pairs(&[("locked/a.lua", "local x=1")]);
    let locked = root.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Privileged test runners can bypass filesystem permissions.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let output = run(root.path(), &["-q", "."], "");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.code(), Some(1));
    assert!(
        output.stderr.contains("could not be read"),
        "{}",
        output.stderr
    );
}

#[test]
fn configs_resolve_nearest_then_xdg_then_home_with_stdin_filename_support() {
    let root = tree_pairs(&[
        ("home/luafmt.dotfile", "luafmt {\nindent = 6\n}"),
        ("config/luafmt/luafmt.dotfile", "luafmt {\nindent = 4\n}"),
        ("sub/luafmt.dotfile", "luafmt {\nindent = 3\n}"),
        ("sub/a.lua", "if x then f() end"),
        ("sub/deep/a.lua", "if x then f() end"),
    ]);
    let input = "if x then f() end";
    let output = run(root.path(), &["--stdin", "sub/deep/new.lua"], input);
    assert!(output.stdout.contains("\n   f()\n"), "{}", output.stderr);
    assert!(
        run(root.path(), &["--stdin", "a.lua"], input)
            .stdout
            .contains("\n    f()\n")
    );
    fs::remove_file(root.path().join("config/luafmt/luafmt.dotfile")).unwrap();
    assert!(
        run(root.path(), &[], input)
            .stdout
            .contains("\n      f()\n")
    );
    let output = run(root.path(), &["-v", "sub"], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert!(output.stderr.contains("luafmt.dotfile"));
    for name in ["sub/a.lua", "sub/deep/a.lua"] {
        assert!(
            fs::read_to_string(root.path().join(name))
                .unwrap()
                .contains("\n   f()\n")
        );
    }
}

#[test]
fn dialect_flag_overrides_config_and_auto_detects_luau() {
    let root = tree_pairs(&[("luafmt.dotfile", "luafmt {\ndialect = lua51\n}")]);
    let input = "local x: number=1";
    assert_eq!(run(root.path(), &[], input).code(), Some(1));
    assert_eq!(
        run(root.path(), &["--dialect", "luau"], input).code(),
        Some(0)
    );
    fs::remove_file(root.path().join("luafmt.dotfile")).unwrap();
    let labels = run(
        root.path(),
        &["--stdin", "labels.lua"],
        "::again:: goto again",
    );
    assert_eq!(labels.code(), Some(0), "{}", labels.stderr);
    let output = run(
        root.path(),
        &["--stdin", "typed.luau"],
        "type T = Foo<Bar<number>>",
    );
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(
        run(root.path(), &["--dialect", "lua52"], "::again:: goto again").code(),
        Some(0)
    );
}

#[test]
fn filters_apply_to_files_and_stdin_and_nested_configs_can_override() {
    let root = tree_pairs(&[
        ("luafmt.dotfile", "blacklist {\n/sub/\n}\n"),
        ("sub/luafmt.dotfile", "blacklist {\n/skip.lua\n}\n"),
        ("sub/skip.lua", "not lua"),
        ("sub/keep.lua", "local x=1"),
    ]);
    assert_eq!(
        run(root.path(), &["--stdin", "sub/skip.lua"], "not lua").stdout,
        "not lua"
    );
    assert_eq!(
        run(
            root.path(),
            &["--check", "--stdin", "sub/skip.lua"],
            "not lua"
        )
        .code(),
        Some(0)
    );
    assert_eq!(run(root.path(), &["."], "").code(), Some(0));
    assert_eq!(
        fs::read_to_string(root.path().join("sub/skip.lua")).unwrap(),
        "not lua"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("sub/keep.lua")).unwrap(),
        "local x = 1"
    );
}

#[test]
fn malformed_input_and_configs_never_overwrite_files_or_stop_other_files() {
    let root = tree_pairs(&[("a.lua", "local ="), ("b.lua", "local b=2")]);
    let output = run(root.path(), &["-q", "."], "");
    assert_eq!(output.code(), Some(1));
    assert!(output.stderr.contains("a.lua"));
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local ="
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local b = 2"
    );
    fs::write(
        root.path().join("luafmt.dotfile"),
        "luafmt {\nwidht = 10\n}",
    )
    .unwrap();
    let output = run(root.path(), &["a.lua"], "");
    assert_eq!(output.code(), Some(1));
    assert!(
        output
            .stderr
            .contains("luafmt.dotfile: line 2: unknown setting: widht")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local ="
    );
    fs::remove_file(root.path().join("luafmt.dotfile")).unwrap();
    fs::write(root.path().join("binary.lua"), [0xff, 0xfe]).unwrap();
    assert_eq!(run(root.path(), &["binary.lua"], "").code(), Some(1));
    assert_eq!(
        fs::read(root.path().join("binary.lua")).unwrap(),
        [0xff, 0xfe]
    );
    assert_eq!(run(root.path(), &["missing.lua"], "").code(), Some(1));
}

#[test]
fn help_completions_and_command_dump_use_shared_cli() {
    let root = tree_pairs(&[]);
    for (args, expected) in [
        (vec!["--help"], "Usage: luafmt"),
        (vec!["--completions", "zsh"], "#compdef luafmt"),
        (vec!["--command-dump"], "\"version\":1"),
    ] {
        let output = run(root.path(), &args, "");
        assert_eq!(output.code(), Some(0), "{}", output.stderr);
        assert!(output.stdout.contains(expected), "{}", output.stdout);
    }
}

#[cfg(unix)]
#[test]
fn replacement_preserves_permissions_symlinks_and_unchanged_inodes() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let root = tree_pairs(&[("a.lua", "local x=1")]);
    let path = root.path().join("a.lua");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("a.lua", root.path().join("link.lua")).unwrap();
    assert_eq!(run(root.path(), &["link.lua"], "").code(), Some(0));
    assert!(root.path().join("link.lua").is_symlink());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "local x = 1");
    let before = fs::metadata(&path).unwrap();
    assert_eq!(run(root.path(), &["a.lua"], "").code(), Some(0));
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
}
