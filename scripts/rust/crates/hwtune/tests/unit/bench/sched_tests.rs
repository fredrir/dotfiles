use super::*;

#[test]
fn wake_arguments_only_differ_in_load() {
    let idle = wake_args(false);
    let loaded = wake_args(true);
    assert_eq!(idle[0], "wake");
    assert_eq!(&idle[..idle.len() - 2], &loaded[..loaded.len() - 2]);
    assert_eq!(&idle[idle.len() - 2..], ["--load", "none"]);
    assert_eq!(&loaded[loaded.len() - 2..], ["--load", "all"]);
}

#[test]
fn sched_jobs_are_family_gated_and_lower_is_better() {
    let setting = Setting {
        tier: "quick".into(),
        workdir: std::env::temp_dir(),
        families: vec!["cpu".into()],
        memory_bytes: 0,
    };
    assert!(jobs(&setting).unwrap().is_empty());
    let setting = Setting {
        families: vec!["sched".into()],
        ..setting
    };
    for job in jobs(&setting).unwrap() {
        assert_eq!(job.name, "sched.wake");
        assert_eq!(job.outputs.len(), 2);
        assert!(job.outputs.iter().all(|o| o.proportion == "LIB"
            && o.scale == "us"
            && o.comparable == "host"
            && o.family() == "sched"));
    }
}
