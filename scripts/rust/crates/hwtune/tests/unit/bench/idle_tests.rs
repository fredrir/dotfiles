use super::*;
use std::fs;
use std::path::Path;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let sys = temp.path();
    let rapl = sys.join("class/powercap/intel-rapl:0");
    write(&rapl.join("name"), "package-0\n");
    write(&rapl.join("energy_uj"), "1000000\n");
    write(&rapl.join("max_energy_range_uj"), "65532610987\n");
    let chip = sys.join("class/hwmon/hwmon0");
    write(&chip.join("name"), "nct6799\n");
    for (channel, rpm) in [(1u8, 596u32), (2, 575), (3, 426), (7, 1496)] {
        write(
            &chip.join(format!("fan{channel}_input")),
            &format!("{rpm}\n"),
        );
        write(&chip.join(format!("pwm{channel}")), "60\n");
    }
    let cpu = sys.join("class/hwmon/hwmon1");
    write(&cpu.join("name"), "k10temp\n");
    write(&cpu.join("temp1_input"), "36500\n");
    temp
}

fn sources(temp: &tempfile::TempDir) -> Sources {
    let sys = Sysfs {
        sys: temp.path().into(),
        dev: temp.path().join("dev"),
    };
    Sources {
        rapl: Rapl::discover(&sys).ok(),
        chip: Hwmon::find(&sys, hwmon::CHIP).ok(),
        cpu: Hwmon::find(&sys, hwmon::CPU_SENSOR).ok(),
        gpu: false,
    }
}

#[test]
fn keys_follow_the_discovered_sources() {
    let temp = fixture();
    let keys = sources(&temp).keys();
    assert_eq!(
        keys.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        vec![PACKAGE, FANS, TCTL]
    );
    let empty = tempfile::tempdir().unwrap();
    assert!(sources(&empty).keys().is_empty());
}

#[test]
fn window_averages_fans_and_temperature_and_converts_energy_to_watts() {
    let temp = fixture();
    let energy = temp.path().join("class/powercap/intel-rapl:0/energy_uj");
    let writer = {
        let energy = energy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            fs::write(energy, "3000000\n").unwrap();
        })
    };
    let measurement = sample_window(
        &sources(&temp),
        Duration::from_millis(300),
        Duration::from_millis(100),
    )
    .unwrap();
    writer.join().unwrap();
    let watts = measurement.values[PACKAGE][0];
    assert!(watts > 0.0 && watts < 20.0, "{watts}");
    assert_eq!(measurement.values[FANS], vec![3093.0]);
    assert_eq!(measurement.values[TCTL], vec![36.5]);
    assert!(!measurement.values.contains_key(GPU));
    assert!(measurement.detail["samples"].as_u64().unwrap() >= 3);
}

#[test]
fn a_window_without_any_source_is_an_error() {
    let empty = tempfile::tempdir().unwrap();
    let error = match sample_window(
        &sources(&empty),
        Duration::from_millis(50),
        Duration::from_millis(10),
    ) {
        Ok(_) => panic!("empty sources measured something"),
        Err(error) => error,
    };
    assert_eq!(error, "no idle sources produced samples");
}
