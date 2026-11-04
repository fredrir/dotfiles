#![forbid(unsafe_code)]

use base64::Engine;
use dotfmt_core::config::Setting;
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_plist::{Output, config::Config, format};

fn configured(settings: &str) -> Config {
    let settings = settings
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(line, text)| {
            let (key, value) = text.split_once('=').unwrap();
            (
                key.trim().to_owned(),
                Setting {
                    value: value.trim().to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: line + 1,
                    global: false,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

fn formatted(input: &str, config: &Config) -> String {
    match format(input.as_bytes(), config).unwrap() {
        Output::Text(text) => text,
        Output::Verbatim => panic!("expected XML output for {input:?}"),
    }
}

fn stable(input: &str, expected: &str, config: &Config) {
    assert_eq!(formatted(input, config), expected, "{input:?}");
    assert_eq!(formatted(expected, config), expected, "{expected:?}");
}

fn syntax(input: &str) -> String {
    match format(input.as_bytes(), &Config::default()) {
        Err(diagnostic) => {
            assert_eq!(diagnostic.kind, DiagnosticKind::Syntax, "{diagnostic:?}");
            assert!(diagnostic.line.is_some(), "{diagnostic:?}");
            diagnostic.message
        }
        Ok(_) => panic!("expected a syntax diagnostic for {input:?}"),
    }
}

#[test]
fn preserves_text_bytes_exactly() {
    stable(
        "<plist version=\"1.0\"><dict><key>  padded key  </key><string>  padded  </string><key>multi</key><string>\n  two\n  lines\n</string></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>  padded key  </key>\n  <string>  padded  </string>\n  <key>multi</key>\n  <string>\n  two\n  lines\n</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn preserves_entity_spellings_and_cdata() {
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><string>&amp;&amp; &lt; &#38; &#x26;</string><key>B</key><string><![CDATA[a < b & c]]></string></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>&amp;&amp; &lt; &#38; &#x26;</string>\n  <key>B</key>\n  <string><![CDATA[a < b & c]]></string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><string>a &amp; b</string></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>a &amp; b</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn keeps_key_order_and_duplicate_keys() {
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><string>first</string><key>B</key><string>middle</string><key>A</key><string>last</string></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>first</string>\n  <key>B</key>\n  <string>middle</string>\n  <key>A</key>\n  <string>last</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn keeps_scalar_spelling() {
    stable(
        "<plist version=\"1.0\"><dict><key>hex</key><integer>0x1f</integer><key>negative</key><integer>-0</integer><key>nan</key><real>nan</real><key>small</key><real>1e-9</real><key>date</key><date>2024-01-02T03:04:05Z</date></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>hex</key>\n  <integer>0x1f</integer>\n  <key>negative</key>\n  <integer>-0</integer>\n  <key>nan</key>\n  <real>nan</real>\n  <key>small</key>\n  <real>1e-9</real>\n  <key>date</key>\n  <date>2024-01-02T03:04:05Z</date>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn preserves_empty_element_forms() {
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><string></string><key>B</key><string/><key>C</key><data></data><key>D</key><data/></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string></string>\n  <key>B</key>\n  <string/>\n  <key>C</key>\n  <data></data>\n  <key>D</key>\n  <data/>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn keeps_comments_in_place() {
    stable(
        "<?xml version=\"1.0\"?>\n<!-- top -->\n<plist version=\"1.0\">\n<dict>\n  <!-- before key -->\n  <key>A</key>\n  <!-- between -->\n  <string>1</string>\n  <!-- at end -->\n</dict>\n<!-- tail -->\n</plist>\n<!-- after -->\n",
        "<?xml version=\"1.0\"?>\n<!-- top -->\n<plist version=\"1.0\">\n<dict>\n  <!-- before key -->\n  <key>A</key>\n  <!-- between -->\n  <string>1</string>\n  <!-- at end -->\n</dict>\n<!-- tail -->\n</plist>\n<!-- after -->\n",
        &Config::default(),
    );
}

#[test]
fn rewraps_data_without_changing_decoded_bytes() {
    let original = b"hello world, this is a property list data payload";
    let encoded = base64::engine::general_purpose::STANDARD.encode(original);
    let input =
        format!("<plist version=\"1.0\"><dict><key>D</key><data>{encoded}</data></dict></plist>\n");
    let config = configured("data_width = 16");
    let output = formatted(&input, &config);
    let start = output.find("<data>").unwrap() + "<data>".len();
    let end = output.find("</data>").unwrap();
    let payload: String = output[start..end]
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap(),
        original
    );
    assert_eq!(formatted(&output, &config), output);
}

#[test]
fn rejects_semantically_invalid_plists() {
    assert!(
        syntax("<plist version=\"1.0\"><true/><false/></plist>\n").contains("exactly one value")
    );
    assert!(syntax("<plist version=\"1.0\"><key>A</key></plist>\n").contains("<dict>"));
    assert!(
        syntax("<plist version=\"1.0\"><dict><string>x</string></dict></plist>\n")
            .contains("<key>")
    );
    assert!(
        syntax("<plist version=\"1.0\"><dict><key>A</key></dict></plist>\n")
            .contains("missing value")
    );
    assert!(
        syntax("<plist version=\"1.0\"><dict>oops</dict></plist>\n")
            .contains("text is not allowed")
    );
    assert!(syntax("<plist version=\"1.0\"><thing/></plist>\n").contains("unknown element"));
    assert!(
        syntax("<plist version=\"1.0\"><array><plist/></array></plist>\n")
            .contains("nested <plist>")
    );
    assert!(
        syntax("<plist version=\"1.0\"><true>yes</true></plist>\n").contains("unexpected content")
    );
    let _ = syntax("<plist version=\"1.0\"><dict>\n");
}

#[test]
fn rejects_nesting_deeper_than_the_limit() {
    let mut input = String::from("<plist version=\"1.0\">");
    for _ in 0..120 {
        input.push_str("<array>");
    }
    input.push_str("<string>x</string>");
    for _ in 0..120 {
        input.push_str("</array>");
    }
    input.push_str("</plist>");
    let message = syntax(&input);
    assert!(message.contains("unsupported nesting deeper than 100 levels"));

    let mut accepted = String::from("<plist version=\"1.0\">");
    for _ in 0..3 {
        accepted.push_str("<array>");
    }
    accepted.push_str("<string>x</string>");
    for _ in 0..3 {
        accepted.push_str("</array>");
    }
    accepted.push_str("</plist>");
    assert!(format(accepted.as_bytes(), &Config::default()).is_ok());
}

#[test]
fn preserves_unicode_and_entity_keys() {
    stable(
        "<plist version=\"1.0\"><dict><key>née &amp; co</key><string>naïve ☕</string></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>née &amp; co</key>\n  <string>naïve ☕</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn keeps_blank_lines_around_comments() {
    stable(
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>1</string>\n\n  <!-- note -->\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>1</string>\n\n  <!-- note -->\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn rejects_malformed_data() {
    let message = syntax("<plist version=\"1.0\"><data>not base64!</data></plist>\n");
    assert!(message.contains("invalid base64"));
}

#[test]
fn reports_error_locations() {
    let error = format(
        b"<plist version=\"1.0\">\n<dict>\n<string>x</string>\n</dict>\n</plist>\n",
        &Config::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(3));
    assert_eq!(error.column, Some(1));
}
