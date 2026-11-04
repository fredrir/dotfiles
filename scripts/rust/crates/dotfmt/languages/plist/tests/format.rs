#![forbid(unsafe_code)]

use dotfmt_core::config::Setting;
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

#[test]
fn leaves_canonical_launchd_plists_untouched() {
    for fixture in [
        include_str!("fixtures/com.fredrir.atuin-server.plist"),
        include_str!("fixtures/com.fredrir.hport.plist"),
    ] {
        stable(fixture, fixture, &Config::default());
    }
}

#[test]
fn indents_one_element_per_line() {
    stable(
        "<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict><key>A</key><string>1</string><key>B</key><array><string>x</string><string>y</string></array></dict></plist>\n",
        "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>1</string>\n  <key>B</key>\n  <array>\n    <string>x</string>\n    <string>y</string>\n  </array>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn applies_configured_indentation() {
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><array><string>x</string></array></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n    <key>A</key>\n    <array>\n        <string>x</string>\n    </array>\n</dict>\n</plist>\n",
        &configured("indent = 4"),
    );
    stable(
        "<plist version=\"1.0\"><dict><key>A</key><array><string>x</string></array></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n\t<key>A</key>\n\t<array>\n\t\t<string>x</string>\n\t</array>\n</dict>\n</plist>\n",
        &configured("indent_type = tabs"),
    );
}

#[test]
fn preserves_attributes_and_their_spelling() {
    stable(
        "<?xml version=\"1.0\"?>\n<plist   version = \"1.0\" ><dict><key>A</key><string>1</string></dict></plist>\n",
        "<?xml version=\"1.0\"?>\n<plist   version = \"1.0\" >\n<dict>\n  <key>A</key>\n  <string>1</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
}

#[test]
fn collapses_blank_runs_between_children() {
    stable(
        "<plist version=\"1.0\">\n<dict>\n\n\n  <key>A</key>\n  <string>1</string>\n\n\n\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>1</string>\n\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
    stable(
        "<plist version=\"1.0\">\n<dict>\n\n\n  <key>A</key>\n  <string>1</string>\n\n\n\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <string>1</string>\n  <key>B</key>\n  <string>2</string>\n</dict>\n</plist>\n",
        &configured("blank_lines = 0"),
    );
}

#[test]
fn changes_only_layout_line_endings() {
    let input = "<?xml version=\"1.0\"?>\r\n<plist version=\"1.0\">\r\n<dict>\r\n  <key>S</key>\r\n  <string>a\r\nb</string>\r\n</dict>\r\n</plist>\r\n";
    stable(
        input,
        "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>S</key>\n  <string>a\r\nb</string>\n</dict>\n</plist>\n",
        &Config::default(),
    );
    stable(
        input,
        "<?xml version=\"1.0\"?>\r\n<plist version=\"1.0\">\r\n<dict>\r\n  <key>S</key>\r\n  <string>a\r\nb</string>\r\n</dict>\r\n</plist>\r\n",
        &configured("line_endings = windows"),
    );
}

#[test]
fn preserves_a_byte_order_mark() {
    let mut input = b"\xef\xbb\xbf".to_vec();
    input.extend_from_slice(b"<plist version=\"1.0\"><true/></plist>");
    let Output::Text(output) = format(&input, &Config::default()).unwrap() else {
        panic!("expected XML output");
    };
    assert!(output.starts_with('\u{feff}'));
    assert_eq!(
        output,
        "\u{feff}<plist version=\"1.0\">\n<true/>\n</plist>\n"
    );
}

#[test]
fn writes_or_omits_the_final_newline() {
    stable(
        "<plist version=\"1.0\"><true/></plist>",
        "<plist version=\"1.0\">\n<true/>\n</plist>\n",
        &Config::default(),
    );
    stable(
        "<plist version=\"1.0\"><true/></plist>\n",
        "<plist version=\"1.0\">\n<true/>\n</plist>",
        &configured("final_newline = false"),
    );
}

#[test]
fn normalizes_the_prolog_only_when_asked() {
    stable(
        "<?xml version='1.0'?>\n<plist version=\"1.0\"><true/></plist>\n",
        "<?xml version='1.0'?>\n<plist version=\"1.0\">\n<true/>\n</plist>\n",
        &Config::default(),
    );
    stable(
        "<?xml version='1.0'?>\n<plist version=\"1.0\"><true/></plist>\n",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<true/>\n</plist>\n",
        &configured("prolog = normalize"),
    );
    stable(
        "<!DOCTYPE plist SYSTEM \"ignored\">\n<plist version=\"1.0\"><true/></plist>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<true/>\n</plist>\n",
        &configured("prolog = normalize"),
    );
}

#[test]
fn normalizes_empty_elements_only_when_asked() {
    let input = "<plist version=\"1.0\"><dict><key>A</key><dict></dict><key>B</key><true/></dict></plist>\n";
    stable(
        input,
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <dict></dict>\n  <key>B</key>\n  <true/>\n</dict>\n</plist>\n",
        &Config::default(),
    );
    stable(
        input,
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <dict/>\n  <key>B</key>\n  <true/>\n</dict>\n</plist>\n",
        &configured("self_closing = always"),
    );
    stable(
        input,
        "<plist version=\"1.0\">\n<dict>\n  <key>A</key>\n  <dict></dict>\n  <key>B</key>\n  <true></true>\n</dict>\n</plist>\n",
        &configured("self_closing = never"),
    );
}

#[test]
fn preserves_wrapped_data_until_a_width_is_configured() {
    let input = "<plist version=\"1.0\"><dict><key>D</key><data>\n    aGVs\n    bG8=\n  </data></dict></plist>\n";
    stable(
        input,
        "<plist version=\"1.0\">\n<dict>\n  <key>D</key>\n  <data>\n    aGVs\n    bG8=\n  </data>\n</dict>\n</plist>\n",
        &Config::default(),
    );
    stable(
        "<plist version=\"1.0\"><dict><key>D</key><data>aGVsbG8gd29ybGQ=</data></dict></plist>\n",
        "<plist version=\"1.0\">\n<dict>\n  <key>D</key>\n  <data>\n    aGVsbG8g\n    d29ybGQ=\n  </data>\n</dict>\n</plist>\n",
        &configured("data_width = 8"),
    );
}

#[test]
fn reformats_launchd_plists_idempotently_under_other_settings() {
    for fixture in [
        include_str!("fixtures/com.fredrir.atuin-server.plist"),
        include_str!("fixtures/com.fredrir.hport.plist"),
    ] {
        for settings in [
            "indent = 4",
            "indent_type = tabs",
            "line_endings = windows",
            "prolog = normalize",
            "self_closing = always",
            "self_closing = never",
            "blank_lines = 0",
        ] {
            let config = configured(settings);
            let output = formatted(fixture, &config);
            assert_eq!(
                formatted(&output, &config),
                output,
                "{settings} changed a launchd plist on the second pass"
            );
        }
    }
}

#[test]
fn agrees_with_an_independent_plist_reader() {
    for fixture in [
        include_str!("fixtures/com.fredrir.atuin-server.plist"),
        include_str!("fixtures/com.fredrir.hport.plist"),
    ] {
        let before = plist::Value::from_reader_xml(fixture.as_bytes()).unwrap();
        let output = formatted(fixture, &Config::default());
        let after = plist::Value::from_reader_xml(output.as_bytes()).unwrap();
        assert_eq!(before, after);
    }
}
