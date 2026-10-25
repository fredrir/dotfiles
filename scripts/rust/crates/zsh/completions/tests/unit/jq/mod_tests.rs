use std::path::{Path, PathBuf};

use super::*;
use testkit::tree;

const DATA: &str =
    r#"data.json={"items":[{"name":"x","id":1},{"name":"y","tags":[]}],"my key":"v"}"#;

fn system_jq() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("jq"))
        .find(|path| path.is_file())
        .expect("jq on PATH")
}

// A working directory with `data.json` and the system jq on PATH.
fn install(files: &[&str]) -> testkit::TempDir {
    let mut specs = vec!["bin/", DATA];
    specs.extend_from_slice(files);
    let root = tree(&specs);
    std::os::unix::fs::symlink(system_jq(), root.path().join("bin/jq")).unwrap();
    root
}

fn context(root: &Path) -> Context {
    let path = root.join("bin").to_string_lossy().to_string();
    Context::testing(root, root, &[("PATH", &path)])
}

// `words` as zsh passes them, quotes included; the cursor is on the word at `current`.
fn completed_at(root: &Path, words: &[&str], current: usize, prefix: &str) -> Reply {
    let words = words.iter().map(|word| (*word).to_string()).collect();
    complete(
        &context(root),
        &Line::new(words, current + 1, Some(prefix.into())),
    )
}

fn completed(root: &Path, words: &[&str], prefix: &str) -> Reply {
    completed_at(root, words, words.len() - 1, prefix)
}

#[test]
fn keys_come_from_the_input_files_around_the_filter() {
    let root = install(&[]);
    let reply = completed_at(
        root.path(),
        &["jq", "'.items[] | select(.na", "data.json"],
        1,
        ".items[] | select(.na",
    );
    let plain = reply.plain();
    assert!(plain.starts_with("skip\t.items[] | select(.\n"), "{plain}");
    assert_eq!(reply.values(), ["name", "id", "tags"]);
    assert!(
        plain.contains("item\tname\tname  string  \"x\"\n"),
        "{plain}"
    );
    assert!(
        plain.contains("item\ttags\ttags  array   0 items\n"),
        "{plain}"
    );
}

#[test]
fn keys_that_are_not_identifiers_are_inserted_in_quotes() {
    let root = install(&[]);
    let reply = completed_at(root.path(), &["jq", "-r", "'.my", "data.json"], 2, ".my");
    assert!(
        reply
            .plain()
            .contains("group\tkeys\tkey\tlines\tunsorted\tsuffix=\"\tprefix=\"\nitem\tmy key\t"),
        "{}",
        reply.plain()
    );
    let quoted = completed_at(root.path(), &["jq", "'.\"my", "data.json"], 1, ".\"my");
    assert!(quoted.plain().contains("suffix=\""), "{}", quoted.plain());
    assert!(quoted.values().contains(&"my key"));
}

#[test]
fn a_new_term_offers_paths_functions_and_keywords() {
    let root = install(&[]);
    let reply = completed_at(
        root.path(),
        &["jq", "'.items[] | ", "data.json"],
        1,
        ".items[] | ",
    );
    let values = reply.values();
    assert_eq!(values[..3], [".name", ".id", ".tags"]);
    for value in ["map", "select", "length", "if", "reduce"] {
        assert!(values.contains(&value), "{value}");
    }
    let plain = reply.plain();
    assert!(plain.contains("item\tmap\tmap"), "{plain}");
    assert!(plain.contains("/1"), "{plain}");
    assert!(plain.contains("apply f to each element"), "{plain}");
    assert!(plain.contains("suffix=(\tremovable"), "{plain}");
}

#[test]
fn functions_defined_in_the_filter_come_first() {
    let root = install(&[]);
    let reply = completed(
        root.path(),
        &["jq", "'def pick_name: .name; pi"],
        "def pick_name: .name; pi",
    );
    assert_eq!(reply.values().first(), Some(&"pick_name"));
    assert!(reply.plain().contains("defined in the filter"));
}

#[test]
fn keys_come_from_what_is_piped_or_redirected_into_jq() {
    let root = install(&[]);
    let line = |left: &str, right: &str| {
        Line::new(vec!["jq".into(), "'.".into()], 2, Some(".".into()))
            .with_buffer(left.into(), right.into())
    };
    let ctx = context(root.path());
    let piped = complete(&ctx, &line("cat data.json | jq .", ""));
    assert_eq!(piped.values()[..2], ["items", "my key"]);
    let redirected = complete(&ctx, &line("jq .", " < data.json"));
    assert_eq!(redirected.values()[..2], ["items", "my key"]);
    let echoed = complete(&ctx, &line(r#"echo '{"zz":1}' | jq ."#, ""));
    assert_eq!(echoed.values(), ["zz"]);
}

#[test]
fn without_input_no_keys_are_read() {
    let root = install(&[]);
    let reply = completed(root.path(), &["jq", "'."], ".");
    assert_eq!(reply.values(), Vec::<&str>::new());
}

#[test]
fn null_input_and_variables_reach_the_probe() {
    let root = install(&[]);
    let reply = completed_at(
        root.path(),
        &["jq", "-n", "--argjson", "x", r#"'{"k":1}'"#, r#""\$x | .""#],
        5,
        r"\$x | .",
    );
    assert!(
        reply.plain().starts_with("skip\t\\$x | .\n"),
        "{}",
        reply.plain()
    );
    assert_eq!(reply.values(), ["k"]);
}

#[test]
fn environment_keys_hide_their_values() {
    let root = install(&[]);
    let reply = completed(root.path(), &["jq", "-n", "'$ENV."], "$ENV.");
    assert!(reply.values().contains(&"PATH"));
    let path = root.path().join("bin").to_string_lossy().to_string();
    assert!(!reply.plain().contains(&path), "{}", reply.plain());
}

#[test]
fn variables_include_bindings_named_arguments_and_builtins() {
    let root = install(&[]);
    let reply = completed(
        root.path(),
        &["jq", "--arg", "who", "me", "'. as $item | $"],
        ". as $item | $",
    );
    assert_eq!(
        reply.values(),
        ["$item", "$who", "$ENV", "$ARGS", "$__loc__"]
    );
}

#[test]
fn imported_modules_offer_their_functions() {
    let root = install(&["lib/util.jq=def greet(x): x; def bye: 1;"]);
    let filter = r#"import "util" as u; u::"#;
    let reply = completed(
        root.path(),
        &["jq", "-L", "lib", &format!("'{filter}")],
        filter,
    );
    assert_eq!(reply.values(), ["u::bye", "u::greet"]);
    let modules = completed(root.path(), &["jq", "-Llib", "'import \""], "import \"");
    assert_eq!(modules.values(), ["util"]);
}

#[test]
fn flags_leave_out_used_conflicting_and_standalone_ones() {
    let root = install(&[]);
    let values = completed(root.path(), &["jq", "-nr", "--tab", "-"], "-")
        .values()
        .join(" ");
    for gone in [
        "-n",
        "-r",
        "-j",
        "--raw-output0",
        "--indent",
        "-c",
        "--version",
    ] {
        assert!(
            !values.split(' ').any(|value| value == gone),
            "{gone}: {values}"
        );
    }
    for kept in ["--arg", "-s", "-L"] {
        assert!(
            values.split(' ').any(|value| value == kept),
            "{kept}: {values}"
        );
    }
    assert!(
        completed(root.path(), &["jq", "-"], "-")
            .values()
            .contains(&"--version")
    );
    let repeated = completed(root.path(), &["jq", "--arg", "a", "b", "-"], "-");
    assert!(repeated.values().contains(&"--arg"));
}

#[test]
fn flag_values_complete_by_kind() {
    let root = install(&[]);
    let at = |words: &[&str]| completed(root.path(), words, "").plain();
    assert!(at(&["jq", "--indent", ""]).contains("item\t7\t7\n"));
    assert_eq!(at(&["jq", "-f", ""]), "files\t*.jq(-.)\n");
    assert_eq!(at(&["jq", "-L", ""]), "dirs\n");
    assert_eq!(at(&["jq", "--rawfile", "x", ""]), "files\n");
    assert_eq!(at(&["jq", "--argjson", "x", ""]), "message\tJSON value\n");
    assert_eq!(
        at(&["jq", "--slurpfile", "x", ""]),
        format!("files\t{JSON_FILES}\n")
    );
    let names = completed_at(
        root.path(),
        &["jq", "--arg", "", "v", "'$who + $x | $ENV'"],
        2,
        "",
    );
    assert_eq!(names.values(), ["who", "x"]);
    assert!(names.plain().ends_with("message\tvariable name\n"));
}

#[test]
fn positional_words_after_the_filter_follow_jq() {
    let root = install(&[]);
    let at = |words: &[&str]| completed(root.path(), words, "").plain();
    assert_eq!(at(&["jq", ".", ""]), format!("files\t{JSON_FILES}\n"));
    assert_eq!(
        at(&["jq", "-f", "p.jq", ""]),
        format!("files\t{JSON_FILES}\n")
    );
    assert_eq!(at(&["jq", "--args", ".", ""]), "message\tstring argument\n");
    assert_eq!(
        at(&["jq", ".", "--jsonargs", ""]),
        "message\tJSON argument\n"
    );
    assert_eq!(at(&["jq", "--frobnicate", ""]), "delegate\n");
}

#[test]
fn filters_are_unescaped_as_the_shell_quoted_them() {
    let offsets = |prefix: &str, raw: &str| unescape(prefix, raw);
    assert_eq!(
        offsets(r"\$x.", r#""\$x."#),
        ("$x.".into(), vec![0, 2, 3, 4])
    );
    assert_eq!(offsets(r"\$x", "'\\$x").0, r"\$x");
    assert_eq!(offsets(r".a\ b", r".a\ b").0, ".a b");
}

#[test]
fn the_line_around_the_word_drops_the_word_zsh_unquoted() {
    assert_eq!(
        around("cat a | jq .f", "oo| < x.json", ".foo|"),
        ("cat a | jq ", " < x.json")
    );
    assert_eq!(around("jq ", " x", ""), ("jq ", " x"));
}
