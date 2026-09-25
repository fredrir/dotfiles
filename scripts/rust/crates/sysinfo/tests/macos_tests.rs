use super::*;

#[test]
fn typed_sysctl_integers_preserve_width_and_reject_non_numbers() {
    for (value, expected) in [
        (CtlValue::Int(12), 12),
        (CtlValue::Uint(u32::MAX), u32::MAX as u64),
        (CtlValue::Long(i64::MAX), i64::MAX as u64),
        (CtlValue::Ulong(u64::MAX), u64::MAX),
        (CtlValue::S64(99), 99),
        (CtlValue::U64(1 << 40), 1 << 40),
        (CtlValue::S8(1), 1),
        (CtlValue::U8(255), 255),
        (CtlValue::S16(2), 2),
        (CtlValue::U16(65535), 65535),
        (CtlValue::S32(3), 3),
        (CtlValue::U32(4), 4),
    ] {
        assert_eq!(unsigned_value(value), Some(expected));
    }
    for value in [
        CtlValue::Int(-1),
        CtlValue::Long(-1),
        CtlValue::S64(-1),
        CtlValue::S32(-1),
        CtlValue::S16(-1),
        CtlValue::S8(-1),
        CtlValue::None,
        CtlValue::String("1".into()),
        CtlValue::Struct(vec![1]),
    ] {
        assert_eq!(unsigned_value(value), None);
    }
}

#[test]
fn native_sysctl_keys_keep_their_types() {
    assert!(sysctl_u64("hw.memsize").is_some_and(|value| value > 0));
    assert!(sysctl_u64("hw.physicalcpu").is_some_and(|value| value > 0));
    assert!(sysctl_u64("hw.logicalcpu").is_some_and(|value| value > 0));
    assert!(sysctl_string("machdep.cpu.brand_string").is_some_and(|value| !value.is_empty()));
    assert!(sysctl_u64("machdep.cpu.brand_string").is_none());
    assert!(sysctl_string("hw.memsize").is_none());
    assert!(sysctl_u64("dotfiles.test.missing").is_none());
}

#[test]
fn temperature_keeps_duplicate_sensor_readings_and_filters_invalid_samples() {
    let samples = [
        ("pACC duplicate", 30.0),
        ("pACC duplicate", 60.0),
        ("eACC sensor", 90.0),
        ("PMU tdie0", 20.0),
        ("GPU", 120.0),
        ("other PMU tdie0", 130.0),
    ];
    let filtered = samples
        .into_iter()
        .filter(|(name, _)| cpu_sensor(name))
        .map(|(_, value)| value);
    assert_eq!(average_temperature(filtered), Some(50.0));
    assert_eq!(
        average_temperature([f64::NAN, f64::INFINITY, -1.0, 0.0, 150.0]),
        None
    );
    assert_eq!(
        average_temperature([0.0, 37.25, f64::NAN, 150.0]),
        Some(37.25)
    );
    assert_eq!(average_temperature([]), None);
}

#[test]
fn cf_dictionary_values_are_retained_after_the_dictionary_drops() {
    let value = {
        let dict = CFDictionary::from_CFType_pairs(&[(
            CFString::new("name").as_CFType(),
            CFString::new("device").as_CFType(),
        )])
        .into_untyped();
        assert!(dict_value(&dict, "missing").is_none());
        dict_value(&dict, "name")
    };
    assert_eq!(as_string(value).as_deref(), Some("device"));
}

#[test]
fn native_service_guards_cover_empty_and_populated_queries() {
    assert!(matching_services("DotfilesMissingServiceForTest").is_empty());
    for _ in 0..3 {
        for service in matching_services("IOPlatformExpertDevice") {
            assert!(!entry_name(&service).is_empty());
            assert!(conforms_to(&service, "IOPlatformExpertDevice"));
        }
    }
}

#[test]
fn accelerator_clients_name_their_creator_pid() {
    assert_eq!(creator_pid("pid 431, WindowServer"), Some(431));
    assert_eq!(creator_pid("pid 2804, zen"), Some(2804));
    assert_eq!(creator_pid("WindowServer"), None);
}

#[test]
fn accelerator_time_and_footprint_read_without_privileges() {
    assert!(
        gpu_time_by_pid().is_some(),
        "Apple silicon exposes AppUsage"
    );
    assert!(footprint(std::process::id()).is_some_and(|bytes| bytes > 0));
    assert_eq!(footprint(u32::MAX), None);
}
