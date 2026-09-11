use super::*;

#[test]
fn classifies_real_errors_and_ignores_boot_noise() {
    assert_eq!(
        classify("mce: [Hardware Error]: CPU 3: Machine Check: 0 Bank 5"),
        Some(Category::Mce)
    );
    assert_eq!(
        classify("[Hardware Error]: Corrected error, no action required."),
        Some(Category::Hardware)
    );
    assert_eq!(
        classify("NVRM: Xid (PCI:0000:01:00): 79, pid=1"),
        Some(Category::Xid)
    );
    assert_eq!(classify("MCE: In-kernel MCE decoding enabled."), None);
    assert_eq!(classify("mce: CPU0: Thermal monitoring enabled"), None);
}

#[test]
fn counts_sum_categories() {
    let lines = vec![
        "NVRM: Xid 79".to_string(),
        "Machine check events logged".to_string(),
        "nothing".to_string(),
    ];
    let counts = counts(&lines);
    assert_eq!(counts.total(), 2);
    assert_eq!(counts.summary(), "1 mce, 0 hw, 1 xid");
}
