use super::*;
use sysinfo::bench::record::Metric;

#[test]
fn different_methods_do_not_share_a_best_score() {
    let metric = |method: &str, value| Metric {
        key: "cpu.test".into(),
        method: method.into(),
        scale: "ops/s".into(),
        samples: vec![value],
        ..Default::default()
    };
    let old = Run {
        host: "host".into(),
        run_id: "old".into(),
        started: "2025".into(),
        metrics: vec![metric("tool/1.0", 9999.0)],
        ..Default::default()
    };
    let recent = Run {
        run_id: "new".into(),
        started: "2026".into(),
        metrics: vec![metric("tool/2.0", 50.0)],
        ..old.clone()
    };
    let text = render(&[old, recent], &Baselines::new());
    assert!(text.contains("50.0"));
    assert!(!text.contains("9 999"));
    assert_eq!(number(-12345.0), "-12 345");
    assert_eq!(render(&[], &Baselines::new()), "");
}
