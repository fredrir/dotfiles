use super::*;

fn header() -> Header {
    Header {
        mode: Mode::Recv,
        token: [7u8; 16],
        window: Duration::from_millis(1500),
    }
}

#[test]
fn a_header_survives_the_round_trip() {
    let decoded = Header::decode(&header().encode()).unwrap();
    assert_eq!(decoded.mode, Mode::Recv);
    assert_eq!(decoded.token, [7u8; 16]);
    assert_eq!(decoded.window, Duration::from_millis(1500));
}

#[test]
fn every_phase_has_a_code_of_its_own() {
    for mode in [Mode::Ping, Mode::Send, Mode::Recv, Mode::Bye] {
        assert_eq!(Mode::from_code(mode.code()), Some(mode));
    }
    assert_eq!(Mode::from_code(0), None);
    assert_eq!(Mode::from_code(5), None);
}

#[test]
fn a_foreign_connection_is_told_apart_from_an_old_build() {
    let mut bytes = header().encode();
    bytes[..4].copy_from_slice(b"HTTP");
    assert!(Header::decode(&bytes).unwrap_err().contains("is not hwire"));

    let mut bytes = header().encode();
    bytes[4] = VERSION + 1;
    assert!(
        Header::decode(&bytes)
            .unwrap_err()
            .contains("different builds")
    );
}

#[test]
fn a_count_survives_the_round_trip() {
    let counted = Counted {
        bytes: 4_500_000_000,
        elapsed: Duration::from_millis(1000),
    };
    let decoded = Counted::decode(&counted.encode());
    assert_eq!(decoded.bytes, counted.bytes);
    assert_eq!(decoded.elapsed, counted.elapsed);
    assert_eq!(decoded.bits_per_second(), 36_000_000_000.0);
}

#[test]
fn a_count_with_no_time_in_it_is_not_a_division_by_zero() {
    assert_eq!(Counted::default().bits_per_second(), 0.0);
}

#[test]
fn tokens_are_hex_both_ways() {
    let token = token().unwrap();
    assert_eq!(unhex(&hex(&token)).unwrap(), token);
    assert_eq!(hex(&[0u8; 16]).len(), 32);
    assert!(unhex("abc").is_err());
    assert!(unhex(&"z".repeat(32)).is_err());
}

#[test]
fn two_tokens_are_not_the_same_token() {
    assert_ne!(token().unwrap(), token().unwrap());
}
