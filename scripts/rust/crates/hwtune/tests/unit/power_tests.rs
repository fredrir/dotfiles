use super::*;
use std::fs;

fn fixture(name: &str, energy: &str, range: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("class/powercap/intel-rapl:0");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("name"), name).unwrap();
    fs::write(dir.join("energy_uj"), energy).unwrap();
    fs::write(dir.join("max_energy_range_uj"), range).unwrap();
    temp
}

fn sysfs(temp: &tempfile::TempDir) -> Sysfs {
    Sysfs {
        sys: temp.path().into(),
        dev: temp.path().join("dev"),
    }
}

#[test]
fn package_domain_is_discovered_and_measured() {
    let temp = fixture("package-0\n", "1000000\n", "65532610987\n");
    let rapl = Rapl::discover(&sysfs(&temp)).unwrap();
    let dir = temp.path().join("class/powercap/intel-rapl:0");
    let (value, energy) = rapl
        .measure(|| {
            fs::write(dir.join("energy_uj"), "3500000\n").unwrap();
            7
        })
        .unwrap();
    assert_eq!(value, 7);
    assert!((energy.joules - 2.5).abs() < 1e-9);
    assert!(energy.watts() > 0.0);
}

#[test]
fn counter_wrap_and_missing_or_foreign_domains_are_handled() {
    assert_eq!(wrapped_delta(10, 30, 100), 20);
    assert_eq!(wrapped_delta(90, 10, 100), 20);
    let temp = fixture("core\n", "1\n", "100\n");
    assert!(Rapl::discover(&sysfs(&temp)).is_err());
    let temp = fixture("package-0\n", "1\n", "0\n");
    assert!(Rapl::discover(&sysfs(&temp)).is_err());
    let empty = tempfile::tempdir().unwrap();
    assert!(Rapl::discover(&sysfs(&empty)).is_err());
}
