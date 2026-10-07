use crate::number::canonical;

fn written(literal: &str) -> String {
    canonical(literal).unwrap_or_else(|| panic!("{literal} is a number"))
}

// Every expectation below is what `jq .` answers for the same literal, taken
// from jq 1.7.1: a formatter that disagreed with it would rewrite numbers
// across a repository nobody had touched.

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
        assert_eq!(written(literal), expected, "{literal}");
    }
}

#[test]
fn an_exponent_is_placed_the_way_decnumber_places_it() {
    // Plain while the point lands within six places of the coefficient,
    // scientific otherwise, with the sign of the exponent always written.
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
    // jq's grammar is JSON's with the shape of a number relaxed, so a file that
    // read before this existed still has to read.
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
fn what_is_not_a_number_is_refused() {
    for literal in [
        "", "-", "+", ".", "1e", "1e+", "0x1f", "1_000", "1.2.3", "0.5.5", "01a", "true", "1 ",
        " 1", "1e2e3",
    ] {
        assert_eq!(canonical(literal), None, "{literal} is not a number");
    }
}
