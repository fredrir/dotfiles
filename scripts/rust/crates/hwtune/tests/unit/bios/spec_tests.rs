use super::*;

const SPEC: &str = "live {\n  base_boost_mhz = 5200\n  gpu = 0000:01:00.0\n}\n\n# menu\npbo {\n  Platform Thermal Throttle Limit    = Manual\n  Platform Thermal Throttle Limit#2  = 85\n  Max CPU Boost Clock Override(+)    = Auto\n}";

#[test]
fn keys_may_select_an_occurrence() {
    assert_eq!(parse_key("Name#2"), ("Name".into(), Some(2)));
    assert_eq!(parse_key("Name"), ("Name".into(), None));
    assert_eq!(parse_key("C#"), ("C#".into(), None));
    assert_eq!(parse_key("Name#0"), ("Name#0".into(), None));
}

#[test]
fn parses_live_block_and_expectations() {
    let spec = parse(SPEC).unwrap();
    assert_eq!(spec.live.base_boost_mhz, Some(5200));
    assert_eq!(spec.live.memory_mts, None);
    assert_eq!(spec.live.gpu.as_deref(), Some("0000:01:00.0"));
    assert_eq!(spec.expectations.len(), 3);
    assert_eq!(spec.expectations[1].occurrence, Some(2));
    assert_eq!(spec.expectations[1].section, "pbo");
    assert_eq!(spec.value("Max CPU Boost Clock Override(+)"), Some("Auto"));
}

#[test]
fn rejects_duplicates_missing_values_and_unknown_live_keys() {
    assert!(
        parse("a {\n  X = 1\n  X = 2\n}")
            .unwrap_err()
            .contains("listed twice")
    );
    assert!(parse("a {\n  X\n}").unwrap_err().contains("no value"));
    assert!(
        parse("live {\n  foo = 1\n}")
            .unwrap_err()
            .contains("unknown live key")
    );
    assert!(
        parse("live {\n  memory_mts = fast\n}")
            .unwrap_err()
            .contains("not a number")
    );
}
