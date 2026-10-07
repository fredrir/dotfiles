use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::dialect::Dialect;
use crate::native::{Done, apply, format};
use crate::repair::Repair;

fn written(body: &str) -> String {
    format(
        "stdin",
        body.as_bytes(),
        &Config::default(),
        false,
        Dialect::Json,
    )
    .unwrap_or_else(|message| panic!("{message}"))
    .text
}

fn refused(body: &str) -> String {
    match format(
        "stdin",
        body.as_bytes(),
        &Config::default(),
        false,
        Dialect::Json,
    ) {
        Ok(formatted) => panic!("expected a refusal, got {:?}", formatted.text),
        Err(message) => message,
    }
}

fn refused_even_for_an_editor(body: &str) -> String {
    match format(
        "stdin",
        body.as_bytes(),
        &Config::default(),
        true,
        Dialect::Json,
    ) {
        Ok(formatted) => panic!("expected a refusal, got {:?}", formatted.text),
        Err(message) => message,
    }
}

fn tree(entries: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, body) in entries {
        let path = root.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
    }
    root
}

#[test]
fn a_body_is_laid_out() {
    assert_eq!(written("{\"a\":1}"), "{\n  \"a\": 1\n}\n");
}

#[test]
fn a_body_that_is_already_laid_out_is_returned_as_it_was() {
    let body = "{\n  \"a\": [\n    1\n  ]\n}\n";

    assert_eq!(written(body), body);
}

#[test]
fn an_empty_file_becomes_an_empty_file_rather_than_a_failure() {
    assert_eq!(written(""), "");
}

#[test]
fn a_body_that_holds_no_value_is_refused_rather_than_emptied() {
    // jq answers a file of whitespace with nothing, and a formatter that wrote
    // nothing over a file somebody wrote is one nobody could leave on save.
    assert_eq!(refused("   \n\n"), "stdin:1:1: expected a value");
    assert_eq!(
        refused("// just a note\n"),
        "stdin:1:1: comments are not JSON; --editor fixes this"
    );
    // With the flag the comment goes, and what is left is the same nothing.
    assert_eq!(
        refused_even_for_an_editor("// just a note\n"),
        "stdin:1:1: expected a value"
    );
}

#[test]
fn what_jq_refuses_is_refused_with_the_position_it_is_at() {
    assert_eq!(
        refused("{\n  \"a\": 1,\n}"),
        "stdin:3:1: stray comma; --editor fixes this"
    );
    assert_eq!(
        refused("[1,]"),
        "stdin:1:4: stray comma; --editor fixes this"
    );
}

#[test]
fn what_no_flag_can_take_is_a_failure_with_nothing_on_stdout() {
    // The other half of the editor contract: a body that cannot be read has to
    // fail, so the buffer keeps what it had.
    assert_eq!(refused("{\"a\" 1}"), "stdin:1:6: expected : after a key");
    assert_eq!(
        refused_even_for_an_editor("{\"a\" 1}"),
        "stdin:1:6: expected : after a key"
    );
}

#[test]
fn the_repairs_are_counted_for_the_caller_to_report() {
    let formatted = format(
        "stdin",
        b"{\n  'a': 1, // why\n}\n",
        &Config::default(),
        true,
        Dialect::Json,
    )
    .expect("the body reads with the flag");

    assert_eq!(formatted.text, "{\n  \"a\": 1\n}\n");
    assert_eq!(formatted.repairs.of(Repair::Quote), 1);
    assert_eq!(formatted.repairs.of(Repair::Comment), 1);
}

#[test]
fn a_body_is_written_beside_itself_and_the_old_one_is_gone() {
    let root = tree(&[("a.json", "{\"a\":1}")]);
    let path = root.path().join("a.json");

    let outcome = apply(
        &path,
        "a.json",
        &Config::default(),
        false,
        true,
        Dialect::Json,
    )
    .unwrap();

    assert_eq!(outcome.done, Done::Changed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 1\n}\n");
    assert_eq!(leftovers(root.path()), Vec::<String>::new());
}

#[test]
fn check_writes_nothing_and_still_says_what_would_change() {
    let root = tree(&[("a.json", "{\"a\":1}")]);
    let path = root.path().join("a.json");

    let outcome = apply(
        &path,
        "a.json",
        &Config::default(),
        false,
        false,
        Dialect::Json,
    )
    .unwrap();

    assert_eq!(outcome.done, Done::Changed);
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}");
}

#[test]
fn check_counts_the_repairs_it_would_have_made() {
    let root = tree(&[("a.json", "{\n  \"a\": 1,\n}\n")]);
    let path = root.path().join("a.json");

    let outcome = apply(
        &path,
        "a.json",
        &Config::default(),
        true,
        false,
        Dialect::Json,
    )
    .unwrap();

    assert_eq!(outcome.done, Done::Changed);
    assert_eq!(outcome.repairs.of(Repair::Comma), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 1,\n}\n");
}

#[test]
fn a_body_that_needs_nothing_is_left_alone() {
    let body = "{\n  \"a\": 1\n}\n";
    let root = tree(&[("a.json", body)]);
    let path = root.path().join("a.json");

    let outcome = apply(
        &path,
        "a.json",
        &Config::default(),
        false,
        true,
        Dialect::Json,
    )
    .unwrap();

    assert_eq!(outcome.done, Done::Unchanged);
    assert_eq!(fs::read_to_string(&path).unwrap(), body);
}

#[test]
fn the_mode_of_the_file_travels_with_its_contents() {
    // A rename swaps the inode, so a file that was executable has to be made
    // executable again or formatting it breaks it.
    use std::os::unix::fs::PermissionsExt;

    let root = tree(&[("hook.json", "{\"a\":1}")]);
    let path = root.path().join("hook.json");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();

    apply(
        &path,
        "hook.json",
        &Config::default(),
        false,
        true,
        Dialect::Json,
    )
    .unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn a_symlink_is_formatted_where_it_points_rather_than_replaced() {
    let root = tree(&[("real/target.json", "{\"a\":1}")]);
    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(root.path().join("real/target.json"), &link).unwrap();

    apply(
        &link,
        "link.json",
        &Config::default(),
        false,
        true,
        Dialect::Json,
    )
    .unwrap();

    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(root.path().join("real/target.json")).unwrap(),
        "{\n  \"a\": 1\n}\n"
    );
}

fn leftovers(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    names.sort();
    names
}
