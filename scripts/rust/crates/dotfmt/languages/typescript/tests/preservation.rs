#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_typescript::config::{Config, LineEndings};
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::format;

fn format_with(path: &str, input: &str, config: &Config) -> String {
    format(Path::new(path), input, config, Dialect::Auto).unwrap()
}

#[test]
fn bom_is_preserved() {
    let output = format_with("a.ts", "\u{feff}const x=1", &Config::default());
    assert_eq!(output, "\u{feff}const x = 1;\n");
    assert_eq!(format_with("a.ts", &output, &Config::default()), output);
}

#[test]
fn bom_only_and_empty_inputs_stay_empty() {
    assert_eq!(format_with("a.ts", "", &Config::default()), "");
    assert_eq!(format_with("a.ts", "   \n\t", &Config::default()), "");
    assert_eq!(
        format_with("a.ts", "\u{feff}", &Config::default()),
        "\u{feff}"
    );
    assert_eq!(
        format_with("a.ts", "\u{feff} \n ", &Config::default()),
        "\u{feff}"
    );
}

#[test]
fn crlf_and_lone_cr_are_normalized_to_the_configured_line_ending() {
    let unix = format_with("a.ts", "const a=1\r\nconst b=2\r\n", &Config::default());
    assert_eq!(unix, "const a = 1;\nconst b = 2;\n");
    assert!(!unix.contains('\r'));

    let cr = format_with("a.ts", "const a=1\rconst b=2\r", &Config::default());
    assert_eq!(cr, "const a = 1;\nconst b = 2;\n");

    let config = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    let windows = format_with("a.ts", "const a=1\nconst b=2\n", &config);
    assert_eq!(windows, "const a = 1;\r\nconst b = 2;\r\n");
    assert_eq!(format_with("a.ts", &windows, &config), windows);
}

#[test]
fn final_newline_can_be_disabled() {
    let config = Config {
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(format_with("a.ts", "const x=1", &config), "const x = 1;");
    assert_eq!(
        format_with("a.ts", "const x = 1;\n", &config),
        "const x = 1;"
    );
}

#[test]
fn prettier_ignore_preserves_formatting_and_source_text() {
    let input = "// prettier-ignore\nconst   odd= {a:1,   b:2};\nconst normal={a:1};";
    let output = format_with("a.ts", input, &Config::default());
    assert!(output.contains("const   odd= {a:1,   b:2};"), "{output}");
    assert!(output.contains("const normal = { a: 1 };"), "{output}");
}

#[test]
fn comments_strings_regexes_and_templates_are_not_lost() {
    let input = r#"// leading comment
const url = "https://example.test/a//b";
const re = /a\/b[/]/g;
const t = `line one
line ${1 + 2} /* not a comment */`;
/* trailing block */
export { url, re, t };
"#;
    let output = format_with("a.ts", input, &Config::default());
    for fragment in [
        "// leading comment",
        "https://example.test/a//b",
        "/a\\/b[/]/g",
        "line ${1 + 2} /* not a comment */",
        "/* trailing block */",
        "export { url, re, t };",
    ] {
        assert!(output.contains(fragment), "missing {fragment:?}: {output}");
    }
}

#[test]
fn shebangs_and_directives_survive() {
    let input = "#!/usr/bin/env node\n\"use strict\";\nconst x=1;\nconsole.log(x);\n";
    let output = format_with("a.js", input, &Config::default());
    assert!(output.starts_with("#!/usr/bin/env node\n"), "{output}");
    assert!(output.contains("\"use strict\";"), "{output}");
    assert!(output.contains("const x = 1;"), "{output}");
}

#[test]
fn verification_is_on_and_does_not_reject_normal_code() {
    let config = Config {
        verify: true,
        ..Config::default()
    };
    let input = "const value = { a: 1, b: [2, 3] };\nconsole.log(value);\n";
    assert_eq!(format_with("a.ts", input, &config), input);
}
