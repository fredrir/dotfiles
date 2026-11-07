use crate::number::canonical;

fn written(literal: &str) -> String {
    canonical(literal).unwrap_or_else(|| panic!("{literal} is a number"))
}

fn settled(literal: &str) -> String {
    let once = written(literal);
    assert_eq!(written(&once), once, "{literal} does not settle");
    once
}

#[test]
fn a_coefficient_is_kept_exactly_as_it_was_written() {
    for (literal, expected) in [
        ("1", "1"),
        ("-0", "-0"),
        ("1.0", "1.0"),
        ("1.10", "1.10"),
        ("-0.000", "-0.000"),
        ("0.30000000000000004", "0.30000000000000004"),
        (
            "1.23456789012345678901234567890",
            "1.23456789012345678901234567890",
        ),
        (
            "1234567890123456789012345678901234567890",
            "1234567890123456789012345678901234567890",
        ),
    ] {
        assert_eq!(settled(literal), expected, "{literal}");
    }
}

#[test]
fn an_exponent_is_placed_the_way_decnumber_places_it() {
    for (literal, expected) in [
        ("1e2", "1E+2"),
        ("1E+2", "1E+2"),
        ("0.1e1", "1"),
        ("1e0", "1"),
        ("1E-0", "1"),
        ("10e-7", "0.0000010"),
        ("100e-2", "1.00"),
        ("1e-6", "0.000001"),
        ("1e-7", "1E-7"),
        ("0.0000001", "1E-7"),
        ("1.5e-6", "0.0000015"),
        ("5e-1", "0.5"),
        ("2.5e3", "2.5E+3"),
        ("2e3", "2E+3"),
        ("2000e0", "2000"),
        ("123.456e2", "12345.6"),
        ("1e20", "1E+20"),
        ("1e21", "1E+21"),
        ("1e-1000000", "1E-1000000"),
        ("0.000000000000000000001", "1E-21"),
        ("9.9e307", "9.9E+307"),
    ] {
        assert_eq!(written(literal), expected, "{literal}");
    }
}

#[test]
fn zero_keeps_its_exponent_because_that_is_what_places_the_point() {
    for (literal, expected) in [
        ("0", "0"),
        ("0.0", "0.0"),
        ("0.000", "0.000"),
        ("0e0", "0"),
        ("0e5", "0E+5"),
        ("0E+100", "0E+100"),
        ("0.0e-10", "0E-11"),
        ("0.0e1", "0"),
    ] {
        assert_eq!(written(literal), expected, "{literal}");
    }
}

#[test]
fn jq_reads_a_number_json_would_not_and_normalises_it() {
    for (literal, expected) in [
        ("01", "1"),
        (".5", "0.5"),
        ("+1", "1"),
        ("1.", "1"),
        ("1.e5", "1E+5"),
    ] {
        assert_eq!(written(literal), expected, "{literal}");
    }
}

#[test]
fn an_exponent_that_outruns_a_sixty_four_bit_integer_still_settles() {
    for (literal, expected) in [
        ("1e9223372036854775807", "1E+9223372036854775807"),
        ("1e9223372036854775808", "1E+9223372036854775808"),
        ("1e-9223372036854775807", "1E-9223372036854775807"),
        ("1e-9223372036854775808", "1E-9223372036854775808"),
        ("1e309", "1E+309"),
        ("1E+400", "1E+400"),
        ("0e9223372036854775807", "0E+9223372036854775807"),
        ("0e-9223372036854775807", "0E-9223372036854775807"),
        ("123.45e9223372036854775807", "1.2345E+9223372036854775809"),
    ] {
        assert_eq!(settled(literal), expected, "{literal}");
    }
}

#[test]
fn an_exponent_wider_than_a_hundred_and_twenty_eight_bits_is_left_as_it_was() {
    for literal in [
        "1e999999999999999999999999999999999999999999",
        "-1.0e-999999999999999999999999999999999999999999",
        "0.5E+999999999999999999999999999999999999999999",
    ] {
        assert_eq!(settled(literal), literal);
    }
    assert_eq!(
        canonical("1.e999999999999999999999999999999999999999999"),
        None,
        "a literal that is not JSON cannot stand in for itself"
    );
}

#[test]
fn what_is_already_short_stays_where_it_is() {
    for (literal, expected) in [
        ("1e2", "1E+2"),
        ("1.0e0", "1.0"),
        ("1e0", "1"),
        ("-0", "-0"),
        ("1E+2", "1E+2"),
        ("1e-7", "1E-7"),
    ] {
        assert_eq!(settled(literal), expected, "{literal}");
    }
}

#[test]
fn what_is_not_a_number_is_refused() {
    for literal in [
        "", "-", "+", ".", "1e", "1e+", "0x1f", "1_000", "1.2.3", "0.5.5", "01a", "true", "1 ",
        " 1", "1e2e3",
    ] {
        assert_eq!(canonical(literal), None, "{literal} is not a number");
    }
}
