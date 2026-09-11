use super::*;

#[test]
fn parses_nvidia_smi_csv() {
    let stats = parse_query("24, 11.96, 300.00, 345, 405, 0\n").unwrap();
    assert_eq!(stats.temp_c, 24.0);
    assert_eq!(stats.power_cap_w, 300.0);
    assert_eq!(stats.sm_mhz, 345);
    assert_eq!(stats.fan_pct, 0);
    assert!(parse_query("").is_err());
    assert!(parse_query("24, x, 300, 345, 405, 0").is_err());
}

#[test]
fn bar_sizes_come_from_the_resource_file() {
    let text = "0x00000000fb000000 0x00000000fbffffff 0x0000000000040200\n\
                0x0000006000000000 0x00000063ffffffff 0x000000000014220c\n\
                0x0000000000000000 0x0000000000000000 0x0000000000000000\n";
    assert_eq!(parse_resource(text), vec![16 << 20, 16 << 30]);
}
