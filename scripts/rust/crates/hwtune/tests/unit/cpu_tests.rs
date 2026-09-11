use super::*;
use std::fs;

fn fake(cpus: &[(u32, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (id, siblings) in cpus {
        let dir = root
            .path()
            .join(format!("devices/system/cpu/cpu{id}/topology"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("thread_siblings_list"), format!("{siblings}\n")).unwrap();
    }
    root
}

#[test]
fn physical_cores_take_the_first_sibling() {
    let root = fake(&[(0, "0,8"), (1, "1,9"), (8, "0,8"), (9, "1,9")]);
    let sys = Sysfs {
        sys: root.path().to_path_buf(),
        dev: root.path().to_path_buf(),
    };
    assert_eq!(physical_cores(&sys).unwrap(), vec![0, 1]);
    assert_eq!(logical_count(&sys).unwrap(), 4);
}

#[test]
fn core_lists_accept_ranges_and_commas() {
    assert_eq!(parse_cores("0-3").unwrap(), vec![0, 1, 2, 3]);
    assert_eq!(parse_cores("4,2,2").unwrap(), vec![2, 4]);
    assert_eq!(parse_cores("0-1,7").unwrap(), vec![0, 1, 7]);
    assert!(parse_cores("3-1").is_err());
    assert!(parse_cores("x").is_err());
    assert!(parse_cores("").is_err());
}

#[test]
fn first_sibling_handles_ranges() {
    assert_eq!(first_sibling("0,8"), Some(0));
    assert_eq!(first_sibling("4-5"), Some(4));
    assert_eq!(first_sibling(""), None);
}
