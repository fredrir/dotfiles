#![forbid(unsafe_code)]

use std::fs;
use std::process::Command;

use testkit::{Bin, TempDir, at, stderr, stdout, tree};

const THEME: &str = "background = \"#15152b\"\nnote = \"fixes #123\"\n";
const SWATCHED: &str =
    "background = \"\x1b[38;2;21;21;43m██\x1b[39m #15152b\"\nnote = \"fixes #123\"\n";

fn hexcat() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_hexcat"))
}

fn highlighted() -> Bin {
    hexcat()
        .env("BAT_THEME", "ansi")
        .args(["--color", "always"])
}

// Drops the `ESC [ … m` color codes.
fn visible(output: &str) -> String {
    let mut visible = String::new();
    let mut rest = output;
    while let Some(start) = rest.find('\x1b') {
        visible.push_str(&rest[..start]);
        let end = rest[start..].find('m').expect("color codes end in m");
        rest = &rest[start + end + 1..];
    }
    visible + rest
}

fn themed() -> TempDir {
    let root = tree(&["bin/.keep"]);
    fs::write(at(&root, "theme.toml"), THEME).unwrap();
    root
}

fn system_cat(arguments: &[&str]) -> String {
    let output = Command::new("cat").args(arguments).output().unwrap();
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn piped_output_is_exactly_cat() {
    let root = themed();
    let theme = at(&root, "theme.toml");
    let output = hexcat().arg(&theme).output();
    assert!(output.status.success());
    assert_eq!(stdout(&output), system_cat(&[&theme]));
}

#[test]
fn piped_flags_go_to_cat() {
    let root = themed();
    let theme = at(&root, "theme.toml");
    let output = hexcat().args(["-n", &theme]).output();
    assert_eq!(stdout(&output), system_cat(&["-n", &theme]));
}

#[test]
fn piped_errors_are_cats() {
    let root = themed();
    let missing = at(&root, "nope");
    let output = hexcat().arg(&missing).output();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("cat: {missing}: No such file or directory\n")
    );
}

#[test]
fn color_never_is_exactly_cat() {
    let root = themed();
    let theme = at(&root, "theme.toml");
    let output = hexcat().args(["--color", "never", &theme]).output();
    assert_eq!(stdout(&output), THEME);
}

#[test]
fn colored_output_is_highlighted_with_swatches_and_needs_no_bat_or_cat() {
    let root = themed();
    let output = highlighted()
        .env("PATH", at(&root, "bin"))
        .arg(at(&root, "theme.toml"))
        .output();
    assert!(output.status.success());
    let printed = stdout(&output);
    assert!(printed.contains("\x1b[31mbackground\x1b[0m"));
    assert!(printed.contains("\x1b[38;2;21;21;43m██\x1b[32m #15152b"));
    assert_eq!(
        visible(&printed),
        THEME.replace("\"#15152b", "\"██ #15152b")
    );
}

#[test]
fn bat_theme_picks_the_theme() {
    let root = themed();
    let theme = at(&root, "theme.toml");
    let ansi = hexcat()
        .env("BAT_THEME", "ansi")
        .args(["--color", "always", &theme])
        .output();
    let github = hexcat()
        .env("BAT_THEME", "GitHub")
        .env("COLORTERM", "truecolor")
        .args(["--color", "always", &theme])
        .output();
    assert_ne!(stdout(&ansi), stdout(&github));
    assert_eq!(visible(&stdout(&ansi)), visible(&stdout(&github)));
}

#[test]
fn highlighted_lines_can_be_numbered() {
    let root = themed();
    let output = highlighted()
        .args(["-n", &at(&root, "theme.toml")])
        .output();
    assert_eq!(
        visible(&stdout(&output)),
        "   1 background = \"██ #15152b\"\n   2 note = \"fixes #123\"\n"
    );
}

#[test]
fn highlighting_reads_stdin_without_files_and_for_a_dash() {
    let root = themed();
    let bare = highlighted().stdin("#000\n").output();
    assert_eq!(visible(&stdout(&bare)), "██ #000\n");
    let dashed = highlighted()
        .arg(at(&root, "theme.toml"))
        .arg("-")
        .stdin("#000\n")
        .output();
    assert!(visible(&stdout(&dashed)).ends_with("\"fixes #123\"\n██ #000\n"));
}

#[test]
fn highlighting_reports_a_missing_file_and_prints_the_rest() {
    let root = themed();
    let missing = at(&root, "nope");
    let output = highlighted()
        .args([&missing, &at(&root, "theme.toml")])
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(visible(&stdout(&output)).starts_with("background = "));
    assert_eq!(
        stderr(&output),
        format!("hexcat: {missing}: no such file or directory\n")
    );
}

#[test]
fn highlighting_rejects_a_directory() {
    let root = themed();
    let output = highlighted().arg(root.path()).output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).ends_with(": is a directory\n"));
}

#[test]
fn flags_bat_lacks_go_to_cat_with_swatches() {
    let root = themed();
    let theme = at(&root, "theme.toml");
    let output = hexcat().args(["--color", "always", "-b", &theme]).output();
    let swatch = "\"\x1b[38;2;21;21;43m██\x1b[39m #15152b";
    assert_eq!(
        stdout(&output),
        system_cat(&["-b", &theme]).replace("\"#15152b", swatch)
    );
}

#[test]
fn the_fallback_keeps_cats_exit_status() {
    let root = themed();
    let output = hexcat()
        .args(["--color", "always", "-b"])
        .arg(at(&root, "nope"))
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).starts_with("cat: "));
}

#[test]
fn the_filter_adds_swatches() {
    let root = themed();
    let output = hexcat()
        .args(["--filter", "--color", "always"])
        .arg(at(&root, "theme.toml"))
        .output();
    assert_eq!(stdout(&output), SWATCHED);
}

#[test]
fn the_filter_leaves_piped_output_untouched() {
    let root = themed();
    let output = hexcat()
        .arg("--filter")
        .arg(at(&root, "theme.toml"))
        .output();
    assert_eq!(stdout(&output), THEME);
}

#[test]
fn the_pager_is_skipped_when_piped() {
    let root = themed();
    let output = hexcat()
        .args(["--pager", "--color", "never"])
        .arg(at(&root, "theme.toml"))
        .output();
    assert_eq!(stdout(&output), THEME);
}

#[test]
fn the_filter_reads_stdin_without_files_and_for_a_dash() {
    let root = themed();
    let bare = hexcat().arg("--filter").stdin("#000\n").output();
    assert_eq!(stdout(&bare), "#000\n");
    let dashed = hexcat()
        .arg("--filter")
        .arg(at(&root, "theme.toml"))
        .arg("-")
        .stdin("#000\n")
        .output();
    assert_eq!(stdout(&dashed), format!("{THEME}#000\n"));
}

#[test]
fn the_filter_reports_a_missing_file_and_prints_the_rest() {
    let root = themed();
    let missing = at(&root, "nope");
    let output = hexcat()
        .args(["--filter", &missing])
        .arg(at(&root, "theme.toml"))
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), THEME);
    assert_eq!(
        stderr(&output),
        format!("hexcat: {missing}: no such file or directory\n")
    );
}

#[test]
fn the_filter_rejects_a_directory() {
    let root = themed();
    let output = hexcat().arg("--filter").arg(root.path()).output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).ends_with(": is a directory\n"));
}

#[test]
fn the_filter_keeps_a_last_line_without_a_newline() {
    let root = themed();
    fs::write(at(&root, "tail"), "#000").unwrap();
    let output = hexcat()
        .args(["--filter", "--color", "always"])
        .arg(at(&root, "tail"))
        .output();
    assert_eq!(stdout(&output), "\x1b[38;2;0;0;0m██\x1b[39m #000");
}

#[test]
fn zsh_completions_are_generated() {
    let output = hexcat().args(["--completions", "zsh"]).output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef hexcat"));
}
