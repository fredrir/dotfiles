use super::*;

#[test]
fn a_round_trip_keeps_the_digits_that_change() {
    assert_eq!(milliseconds(Duration::from_micros(184)), "0.184 ms");
    assert_eq!(milliseconds(Duration::from_micros(1_712)), "1.71 ms");
    assert_eq!(milliseconds(Duration::ZERO), "0.000 ms");
}

#[test]
fn a_rate_carries_the_unit_it_is_worth_reading_in() {
    assert_eq!(rate(4_510_000_000.0), "4.51 Gbit/s");
    assert_eq!(rate(940_000_000.0), "940.0 Mbit/s");
    assert_eq!(rate(12_000.0), "12 kbit/s");
    assert_eq!(rate(0.0), "0 kbit/s");
}

#[test]
fn a_column_is_the_same_width_whatever_is_in_it() {
    assert_eq!(column("4.51 Gbit/s").len(), column("940.0 Mbit/s").len());
    assert_eq!(column("0.184 ms").len(), 12);
}

#[test]
fn bytes_per_second_is_the_same_measurement_in_the_other_base() {
    let counted = Counted {
        bytes: 1024 * 1024 * 100,
        elapsed: Duration::from_secs(1),
    };
    assert_eq!(bytes_per_second(&counted), "100 MiB/s");
    assert_eq!(bytes_per_second(&Counted::default()), "0 MiB/s");
}
