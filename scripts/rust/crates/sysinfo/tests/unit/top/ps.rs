use super::*;

#[test]
fn clocks_parse_cpu_time_and_elapsed_forms() {
    assert_eq!(clock("0:05.25"), Some(5.25));
    assert_eq!(clock("1:02.35"), Some(62.35));
    assert_eq!(clock("120:00.00"), Some(7_200.0));
    assert_eq!(clock("01:48:28"), Some(6_508.0));
    assert_eq!(clock("2-01:00:00"), Some(176_400.0));
    assert_eq!(clock("-"), None);
    assert_eq!(clock("1:xx"), None);
}

#[test]
fn rows_keep_executable_paths_with_spaces() {
    let body = "  431     1    88   1:02.35  95584 01:28:23 /System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer\n\
                  546     1     0   0:00.10  16640 2-01:28:22 /Library/Application Support/org.pqrs/Karabiner-Core-Service\n\
                  bad line\n";
    let entries = parse(body);
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[&431],
        Entry {
            parent: 1,
            uid: 88,
            cpu_ms: 62_350.0,
            memory: 95_584 * 1024,
            age: 5_303,
            exe: "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer"
                .into(),
        }
    );
    assert_eq!(
        entries[&546].exe,
        "/Library/Application Support/org.pqrs/Karabiner-Core-Service"
    );
    assert_eq!(entries[&546].age, 2 * 86_400 + 5_302);
}

#[test]
fn deltas_rescale_cpu_time_to_the_sample_window() {
    let started = Instant::now();
    let entry = |cpu_ms| Entry {
        cpu_ms,
        ..Entry::default()
    };
    let before = Reading {
        at: started,
        took: Duration::ZERO,
        entries: HashMap::from([(1, entry(1_000.0)), (2, entry(500.0))]),
    };
    let after = Reading {
        at: started + Duration::from_millis(400),
        took: Duration::ZERO,
        entries: HashMap::from([(1, entry(1_100.0)), (2, entry(400.0)), (3, entry(20.0))]),
    };
    let deltas = deltas(&before, &after, Duration::from_millis(200));
    assert_eq!(deltas[&1].cpu_ms, 50.0);
    assert_eq!(
        deltas[&2].cpu_ms, 0.0,
        "a reused pid never reports negative time"
    );
    assert_eq!(
        deltas[&3].cpu_ms, 10.0,
        "a new process counts all of its time"
    );
}

#[test]
fn no_hidden_processes_means_no_probe() {
    assert!(read(&[]).is_none());
}
