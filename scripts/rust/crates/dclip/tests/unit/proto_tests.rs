use super::*;

fn reply(result: Result<String, String>) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_reply(&mut bytes, &result).unwrap();
    bytes
}

fn decode(bytes: &[u8]) -> Result<String, String> {
    let header = parse_header(bytes[..HEADER].try_into().unwrap())?;
    read_body(&mut &bytes[HEADER..], &header)
}

#[test]
fn a_paste_request_is_the_version_and_the_op() {
    assert_eq!(REQUEST, [VERSION, b'p']);
    assert!(check_request(REQUEST).is_ok());
}

#[test]
fn an_unknown_request_is_refused() {
    assert!(
        check_request([VERSION, b'x'])
            .unwrap_err()
            .contains("unknown")
    );
    assert!(check_request([9, PASTE]).unwrap_err().contains("version 9"));
}

#[test]
fn text_round_trips_through_a_reply() {
    for text in ["", "one line", "two\nlines\n", "ünïcödé ✓"] {
        assert_eq!(decode(&reply(Ok(text.into()))), Ok(text.into()));
    }
}

#[test]
fn a_server_failure_reaches_the_client_as_its_message() {
    assert_eq!(
        decode(&reply(Err("no Wayland session".into()))),
        Err("no Wayland session".into())
    );
}

#[test]
fn the_header_is_version_status_and_big_endian_length() {
    assert_eq!(
        &reply(Ok("abc".into()))[..HEADER],
        &[VERSION, 0, 0, 0, 0, 3]
    );
}

#[test]
fn a_short_body_is_truncated_not_accepted() {
    let bytes = reply(Ok("abcdef".into()));
    assert_eq!(
        decode(&bytes[..bytes.len() - 2]),
        Err("reply truncated".into())
    );
}

#[test]
fn a_header_beyond_the_limit_or_from_another_version_is_refused() {
    let length = (LIMIT as u32 + 1).to_be_bytes();
    let oversized = [VERSION, 0, length[0], length[1], length[2], length[3]];
    assert!(parse_header(oversized).is_err());
    assert!(parse_header([2, 0, 0, 0, 0, 0]).is_err());
    assert!(parse_header([VERSION, 7, 0, 0, 0, 0]).is_err());
}

#[test]
fn a_body_that_is_not_utf8_is_refused() {
    let header = Header {
        ok: true,
        length: 2,
    };
    assert!(read_body(&mut &[0xff_u8, 0xfe][..], &header).is_err());
}
