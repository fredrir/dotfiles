use super::*;
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: TempDir::new().unwrap(),
        }
    }

    fn write(&self, path: &str, value: &str) {
        let path = self.root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn sys(&self) -> Sysfs {
        Sysfs {
            sys: self.root.path().into(),
            dev: self.root.path().join("dev"),
        }
    }

    fn cpu(&self, driver: &str, temperature: &str, limit: Option<&str>) {
        self.write("class/hwmon/hwmon0/name", driver);
        self.write("class/hwmon/hwmon0/temp1_input", temperature);
        self.write("class/hwmon/hwmon0/temp1_label", "CPU package");
        if let Some(limit) = limit {
            self.write("class/hwmon/hwmon0/temp1_crit", limit);
        }
    }
}

fn evidence() -> Evidence {
    Evidence {
        passed: true,
        reason: None,
        peak_temp_c: -30.0,
        max_temp_c: 90.0,
        samples: 0,
        journal_checked: false,
        elapsed_seconds: 0.0,
    }
}

#[test]
fn cpu_limits_use_hardware_margin_and_never_raise_a_requested_limit() {
    let fixture = Fixture::new();
    fixture.cpu("coretemp", "45000", Some("100000"));
    assert_eq!(sensors(&fixture.sys(), None).unwrap()[0].limit, 95.0);
    assert_eq!(sensors(&fixture.sys(), Some(85.0)).unwrap()[0].limit, 85.0);
    assert_eq!(sensors(&fixture.sys(), Some(105.0)).unwrap()[0].limit, 95.0);
}

#[test]
fn missing_cpu_limit_requires_an_explicit_limit() {
    let fixture = Fixture::new();
    fixture.cpu("k10temp", "45000", None);
    assert!(
        sensors(&fixture.sys(), None)
            .unwrap_err()
            .contains("--max-temp")
    );
    assert_eq!(sensors(&fixture.sys(), Some(85.0)).unwrap().len(), 1);
}

#[test]
fn gpu_temperature_does_not_substitute_for_cpu_telemetry() {
    let fixture = Fixture::new();
    fixture.cpu("amdgpu", "45000", Some("100000"));
    assert!(
        sensors(&fixture.sys(), None)
            .unwrap_err()
            .contains("CPU temperature")
    );
}

#[test]
fn every_cpu_sensor_must_remain_readable_and_below_its_limit() {
    let fixture = Fixture::new();
    fixture.cpu("coretemp", "45000", Some("100000"));
    fixture.write("class/hwmon/hwmon0/temp2_input", "50000");
    fixture.write("class/hwmon/hwmon0/temp2_crit", "90000");
    let sensors = sensors(&fixture.sys(), None).unwrap();
    let mut evidence = evidence();
    sample(&sensors, &mut evidence).unwrap();
    assert_eq!(evidence.peak_temp_c, 50.0);
    assert_eq!(evidence.samples, 1);
    fixture.write("class/hwmon/hwmon0/temp2_input", "85000");
    assert!(
        sample(&sensors, &mut evidence)
            .unwrap_err()
            .contains("reached")
    );
    fs::remove_file(fixture.root.path().join("class/hwmon/hwmon0/temp2_input")).unwrap();
    assert!(sample(&sensors, &mut evidence).is_err());
}

#[test]
fn invalid_temperature_values_cannot_pass_a_trial() {
    let fixture = Fixture::new();
    fixture.cpu("k10temp", "NaN", Some("100000"));
    let sensors = sensors(&fixture.sys(), None).unwrap();
    assert!(sample(&sensors, &mut evidence()).is_err());
    fixture.write("class/hwmon/hwmon0/temp1_input", "250000");
    assert!(sample(&sensors, &mut evidence()).is_err());
    assert!(super::sensors(&fixture.sys(), Some(f64::NAN)).is_err());
}

#[test]
fn empty_or_malformed_journal_does_not_mean_no_hardware_errors() {
    assert!(entries("", None).is_err());
    assert!(entries("No journal files were found.", None).is_err());
    assert!(entries(r#"{"MESSAGE":"hello"}"#, None).is_err());
    assert!(entries(r#"{"__CURSOR":"","MESSAGE":"hello"}"#, None).is_err());
}

#[test]
fn journal_cursor_discontinuity_rejects_incomplete_evidence() {
    let line = r#"{"__CURSOR":"new","MESSAGE":"normal"}"#;
    assert!(
        entries(line, Some("old"))
            .err()
            .unwrap()
            .contains("continuity")
    );
    assert_eq!(entries(line, Some("new")).unwrap().len(), 1);
}

#[test]
fn only_new_hardware_errors_fail_a_monitored_trial() {
    let previous = r#"{"__CURSOR":"before","MESSAGE":"mce: [Hardware Error] old"}"#;
    let healthy = format!(
        "{previous}\n{}",
        r#"{"__CURSOR":"after","MESSAGE":"CPU online"}"#
    );
    assess_journal(&entries(&healthy, Some("before")).unwrap()).unwrap();
    let unhealthy = format!(
        "{previous}\n{}",
        r#"{"__CURSOR":"after","MESSAGE":"mce: [Hardware Error] new"}"#
    );
    assert!(
        assess_journal(&entries(&unhealthy, Some("before")).unwrap())
            .unwrap_err()
            .contains("new hardware error")
    );
}
