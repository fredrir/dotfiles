use super::*;

#[test]
fn stress_ng_arguments_per_profile() {
    assert_eq!(
        args(Profile::AllCore, 16, 20, None),
        [
            "--cpu",
            "16",
            "--cpu-method",
            "matrixprod",
            "--timeout",
            "20m",
            "--metrics-brief"
        ]
    );
    assert_eq!(
        args(Profile::Light, 16, 30, None),
        [
            "--cpu",
            "16",
            "--cpu-load",
            "10",
            "--timeout",
            "30m",
            "--metrics-brief"
        ]
    );
    assert_eq!(
        args(Profile::PerCore, 1, 10, Some(3)),
        [
            "--cpu",
            "1",
            "--taskset",
            "3",
            "--cpu-method",
            "matrixprod",
            "--timeout",
            "10m",
            "--metrics-brief"
        ]
    );
}
