use super::*;
use std::fs;

#[test]
fn duty_rounds_to_the_nearest_percent() {
    assert_eq!(duty_pct(38), 15);
    assert_eq!(duty_pct(51), 20);
    assert_eq!(duty_pct(255), 100);
    assert_eq!(duty_pct(0), 0);
}

#[test]
fn duty_matches_within_one_percent() {
    assert!(duty_matches(38, 15));
    assert!(duty_matches(37, 15));
    assert!(!duty_matches(45, 15));
}

#[test]
fn auto_points_stop_at_the_first_missing_point() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (temp, pwm)) in [(30, 38), (60, 51), (125, 255)].iter().enumerate() {
        let point = index + 1;
        fs::write(
            dir.path().join(format!("pwm2_auto_point{point}_temp")),
            format!("{}\n", temp * 1000),
        )
        .unwrap();
        fs::write(
            dir.path().join(format!("pwm2_auto_point{point}_pwm")),
            format!("{pwm}\n"),
        )
        .unwrap();
    }
    let chip = Hwmon {
        dir: dir.path().to_path_buf(),
    };
    assert_eq!(
        chip.auto_points(2).unwrap(),
        vec![(30, 38), (60, 51), (125, 255)]
    );
    assert!(chip.auto_points(3).is_err());
}
