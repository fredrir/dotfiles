use super::*;

#[test]
fn a_size_carries_the_unit_it_is_worth_reading_in() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(512), "512 B");
    assert_eq!(bytes(2048), "2 KiB");
    assert_eq!(bytes(12_900_000), "12.3 MiB");
    assert_eq!(bytes(3_221_225_472), "3.00 GiB");
}

#[test]
fn a_size_changes_unit_exactly_at_the_boundary() {
    assert_eq!(bytes(1023), "1023 B");
    assert_eq!(bytes(1024), "1 KiB");
    assert_eq!(bytes(1_048_575), "1024 KiB");
    assert_eq!(bytes(1_048_576), "1.0 MiB");
    assert_eq!(bytes(1_073_741_823), "1024.0 MiB");
    assert_eq!(bytes(1_073_741_824), "1.00 GiB");
}
