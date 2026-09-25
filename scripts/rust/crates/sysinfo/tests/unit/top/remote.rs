use super::*;
use crate::top::{Row, Sort};

fn options(sort: Sort, count: usize, split: bool) -> Options {
    Options { sort, count, split }
}

#[test]
fn script_runs_the_installed_binary_with_the_same_view() {
    assert_eq!(
        script(options(Sort::Total, 5, false)),
        r#"exec "${DOTFILES_COMPILED:-$HOME/dotfiles/.bin}/sysinfo" --system --json --number 5"#
    );
    assert_eq!(
        script(options(Sort::Gpu, 12, true)),
        r#"exec "${DOTFILES_COMPILED:-$HOME/dotfiles/.bin}/sysinfo" --system --json --number 12 --gpu --split"#
    );
}

#[test]
fn reports_round_trip_without_local_only_fields() {
    let report = Report {
        schema: SCHEMA,
        host: "archie".into(),
        cores: 16,
        memory: 32 << 30,
        gpu: true,
        rows: vec![Row {
            pid: 42,
            uid: Some(1000),
            user: "fredrir".into(),
            cpu: 6.25,
            cores: 1.0,
            memory: 1 << 30,
            memory_share: 3.125,
            gpu: Some(12.0),
            age: 7_200,
            command: "hport listeners --watch".into(),
            count: 1,
        }],
    };
    let parsed = parse("archie", &serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(parsed.rows[0].uid, None);
    assert_eq!(
        Report {
            rows: vec![Row {
                uid: Some(1000),
                ..parsed.rows[0].clone()
            }],
            ..parsed
        },
        report
    );
}

#[test]
fn invalid_and_future_reports_are_rejected() {
    assert!(parse("archie", b"error: unexpected argument").is_err());
    let future = serde_json::json!({"schema": 2, "host": "archie", "cores": 1, "memory": 1, "gpu": false, "rows": []});
    let error = parse("archie", &serde_json::to_vec(&future).unwrap()).unwrap_err();
    assert_eq!(error, "archie: process report schema 2 unsupported");
}
