use super::*;
use serde_json::json;

fn hit(name: &str, downloads: u64) -> Hit {
    Hit {
        name: name.into(),
        description: String::new(),
        version: String::new(),
        downloads,
    }
}

#[test]
fn the_pool_keeps_each_package_once_most_downloaded_first() {
    let pool = vec![hit("chalk", 5), hit("typescript", 9), hit("chalk", 7)];
    assert_eq!(ranked(pool), [hit("typescript", 9), hit("chalk", 7)]);
}

#[test]
fn a_command_is_declared_by_a_bin_path_or_map() {
    assert!(declares_command(&json!({"bin": "cli.js"})));
    assert!(declares_command(&json!({"bin": {"tsc": "bin/tsc"}})));
    assert!(!declares_command(&json!({"bin": {}})));
    assert!(!declares_command(&json!({"bin": ""})));
    assert!(!declares_command(&json!({"name": "react"})));
}

#[test]
fn offline_nothing_is_built() {
    let root = testkit::tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    assert_eq!(PopularSource.build(&ctx), None);
}
