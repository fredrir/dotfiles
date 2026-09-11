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
