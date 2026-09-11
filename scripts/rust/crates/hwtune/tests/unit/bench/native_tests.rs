use super::*;

#[test]
fn worker_output_yields_a_positive_finite_value() {
    assert_eq!(
        value_of(r#"{"workload":"memory","unit":"ns","value":81.250,"elapsed_s":1.2}"#).unwrap(),
        81.25
    );
    for text in [
        "not json",
        r#"{"value":0}"#,
        r#"{"value":-3.0}"#,
        r#"{"value":"12"}"#,
        r#"{"unit":"ns"}"#,
    ] {
        assert!(value_of(text).is_err(), "{text}");
    }
}

#[test]
fn latency_job_is_lower_is_better_and_family_gated() {
    let setting = Setting {
        tier: "quick".into(),
        workdir: std::env::temp_dir(),
        families: vec!["disk".into()],
        memory_bytes: 0,
    };
    assert!(jobs(&setting).unwrap().is_empty());
    let setting = Setting {
        families: vec!["mem".into()],
        ..setting
    };
    let jobs = jobs(&setting).unwrap();
    let Some(latency) = jobs.iter().find(|job| job.name == "mem.latency") else {
        return;
    };
    assert_eq!(latency.method, "mem.latency/1.0.0");
    assert_eq!(latency.outputs[0].proportion, "LIB");
    assert_eq!(latency.outputs[0].scale, "ns");
    assert_eq!(latency.outputs[0].comparable, "world");
}
