#![forbid(unsafe_code)]

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_plist::config::Config;
use dotfmt_plist::{Output, format};

fn assert_verbatim(input: &[u8]) {
    match format(input, &Config::default()) {
        Ok(Output::Verbatim) => {}
        Ok(Output::Text(text)) => panic!("expected verbatim output, formatted to {text:?}"),
        Err(diagnostic) => panic!("expected verbatim output, got {diagnostic:?}"),
    }
}

#[test]
fn binary_plists_are_verbatim() {
    let mut input = b"bplist00".to_vec();
    input.extend_from_slice(&[0, 0x01, 0x7f, 0xff, 0x00, 0xfe]);
    assert_verbatim(&input);
    assert_verbatim(b"bplist01\x00\xff");
}

#[test]
fn openstep_ascii_plists_are_verbatim() {
    assert_verbatim(b"{ a = 1; b = (x, y); }\n");
    assert_verbatim(b"\n  ( \"a\", \"b\" )\n");
}

#[test]
fn empty_and_non_xml_inputs_are_verbatim() {
    assert_verbatim(b"");
    assert_verbatim(b"   \n\t\r\n");
    assert_verbatim(b"\xef\xbb\xbf");
    assert_verbatim(b"this is not an XML property list\n");
}

#[test]
fn invalid_utf8_xml_is_a_syntax_error() {
    let error = format(b"<plist>\xff</plist>", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(1));
}

#[test]
fn malformed_xml_is_a_syntax_error() {
    let error = format(b"<plist version=\"1.0\"><dict>", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
}
