use super::*;

fn sample(boot_id: &str) -> PerCore {
    PerCore {
        session: "20260913-201403-per-core".into(),
        core: 3,
        started: "2026-09-13T20:44:10".into(),
        offset: Some(-25),
        boot_id: boot_id.into(),
        pid: 41233,
    }
}

#[test]
fn state_round_trips_and_is_consumed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nested/percore.json");
    write(&file, &sample("boot-a")).unwrap();
    assert_eq!(take(&file).unwrap(), Some(sample("boot-a")));
    assert_eq!(take(&file).unwrap(), None);
}

#[test]
fn a_different_boot_means_the_machine_rebooted() {
    assert_eq!(verdict(&sample("boot-a"), "boot-b"), Recovered::Rebooted);
    assert_eq!(verdict(&sample("boot-a"), "boot-a"), Recovered::Interrupted);
}
