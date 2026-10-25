use std::path::PathBuf;

use super::*;
use testkit::tree;

fn system_jq() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("jq"))
        .find(|path| path.is_file())
        .expect("jq on PATH")
}

fn key(name: &str, types: &[&str], preview: &str) -> Key {
    Key {
        name: name.into(),
        types: types.iter().map(|kind| (*kind).to_string()).collect(),
        preview: preview.into(),
    }
}

#[test]
fn keys_from_every_object_merge_in_first_seen_order() {
    let lines = [
        r#"["DEBUG:",{"zcomp":[["name","string","\"x\""],["tags","array",2]]}]"#.to_string(),
        r#"["DEBUG:",{"zcomp":[["id","number","1"],["name","null","null"],["meta","object",1]]}]"#
            .to_string(),
        "not a probe line".to_string(),
    ];
    assert_eq!(
        merge(&lines),
        [
            key("name", &["string", "null"], "\"x\""),
            key("tags", &["array"], "2 items"),
            key("id", &["number"], "1"),
            key("meta", &["object"], "1 key"),
        ]
    );
}

#[test]
fn the_probe_reads_the_keys_reaching_the_cursor() {
    let root = tree(&[r#"data.json={"items":[{"name":"x","id":1},{"name":"y","tags":[]}]}"#]);
    let run = Run {
        files: vec![root.path().join("data.json").to_string_lossy().to_string()],
        ..Run::default()
    };
    let program = Program {
        before: ".items[] | select(".into(),
        after: ")".into(),
    };
    let probed = keys(&system_jq(), root.path(), &run, &program, "");
    assert!(probed.compiled);
    let names: Vec<&str> = probed.keys.iter().map(|key| key.name.as_str()).collect();
    assert_eq!(names, ["name", "id", "tags"]);
}

#[test]
fn input_flags_variables_and_standard_input_are_passed_on() {
    let run = Run {
        flags: vec![
            "--slurp".into(),
            "--argjson".into(),
            "x".into(),
            r#"{"k":1}"#.into(),
        ],
        stdin: Some(b"{\"a\":1}\n{\"a\":2}\n".to_vec()),
        ..Run::default()
    };
    let program = Program {
        before: "[., $x] | .[] | ".into(),
        after: String::new(),
    };
    let root = tree(&[]);
    let probed = keys(&system_jq(), root.path(), &run, &program, "");
    let names: Vec<&str> = probed.keys.iter().map(|key| key.name.as_str()).collect();
    assert_eq!(names, ["k"]);
}

#[test]
fn a_filter_that_does_not_compile_is_reported() {
    let root = tree(&[]);
    let run = Run {
        flags: vec!["--null-input".into()],
        ..Run::default()
    };
    let program = Program {
        before: "if ".into(),
        after: String::new(),
    };
    let probed = keys(&system_jq(), root.path(), &run, &program, "");
    assert!(!probed.compiled);
    assert!(probed.keys.is_empty());
}

#[test]
fn large_objects_report_the_keys_matching_the_typed_word() {
    let root = tree(&[]);
    let run = Run {
        flags: vec!["--null-input".into()],
        ..Run::default()
    };
    let program = Program {
        before: "[range(1000) | {key: \"k\\(.)\", value: .}] | from_entries | ".into(),
        after: String::new(),
    };
    let probed = keys(&system_jq(), root.path(), &run, &program, "K99");
    let names: Vec<&str> = probed.keys.iter().map(|key| key.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "k99", "k990", "k991", "k992", "k993", "k994", "k995", "k996", "k997", "k998", "k999"
        ]
    );
}
