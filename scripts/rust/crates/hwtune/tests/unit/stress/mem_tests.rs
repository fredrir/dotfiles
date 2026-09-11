use super::*;

#[test]
fn available_memory_comes_from_meminfo() {
    assert_eq!(
        available_bytes("MemTotal: 10 kB\nMemAvailable:    2048 kB\n"),
        Some(2048 * 1024)
    );
    assert_eq!(available_bytes("MemTotal: 10 kB\n"), None);
}

#[test]
fn commands_size_the_test_from_available_memory() {
    let (program, args) = command(MemTool::StressNg, 30, 80, 1_000_000_000);
    assert_eq!(program, "stress-ng");
    assert_eq!(args[3], "800000000");
    assert!(args.contains(&"--verify".to_string()));
    let (program, args) = command(MemTool::Memtester, 30, 50, 4 << 30);
    assert_eq!(program, "memtester");
    assert_eq!(args, vec!["2048M", "1"]);
}

#[test]
fn y_cruncher_runs_the_memory_tests_within_the_time_limit() {
    let (program, args) = command(MemTool::YCruncher, 30, 80, 1_000_000_000);
    assert_eq!(program, "y-cruncher");
    assert_eq!(args[0], "stress");
    assert_eq!(args[1], "-M:800000000");
    assert_eq!(args[2], "-D:360");
    assert_eq!(args[3], "-TL:1800");
    assert_eq!(&args[4..], &Y_CRUNCHER_TESTS.map(String::from));
    let (_, short) = command(MemTool::YCruncher, 1, 50, 1 << 30);
    assert_eq!(short[2], "-D:30");
    assert_eq!(test_list(MemTool::YCruncher), "VT3,N63,FFTv4,SFTv4,BBP");
    assert_eq!(test_list(MemTool::StressNg), "all");
}

#[test]
fn y_cruncher_log_errors_fail_the_run() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.log");
    fs::write(&log, "Running VT3...\nPassed\n").unwrap();
    assert_eq!(log_failure(&log), None);
    fs::write(&log, "Running N63...\nErrors Encountered!\n").unwrap();
    assert_eq!(
        log_failure(&log).as_deref(),
        Some("log: Errors Encountered!")
    );
    assert_eq!(log_failure(&temp.path().join("missing.log")), None);
}
