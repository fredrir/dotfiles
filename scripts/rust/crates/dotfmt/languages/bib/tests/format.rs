#![forbid(unsafe_code)]

use dotfmt_bib::config::{Config, IndentType, LineEndings, NameCase};
use dotfmt_bib::format;

fn formatted(input: &str, config: &Config) -> String {
    let output = format(input, config).unwrap();
    assert_eq!(
        format(&output, config).unwrap(),
        output,
        "formatting must settle after one pass"
    );
    output
}

fn default(input: &str) -> String {
    formatted(input, &Config::default())
}

#[test]
fn entries_are_indented_aligned_and_case_normalised() {
    let input = "@ARTICLE { Smith2024 ,\nauthor = \"Smith, John\",\ntitle={A {Nested, Comma} Title},\nyear = 2024,\nmonth = jan,\njournal = {Journal},\nnote = {100\\% sure},\nurl = {https://example.com/%7Euser},\n}\n";
    assert_eq!(
        default(input),
        "@article{Smith2024,\n  author  = \"Smith, John\",\n  title   = {A {Nested, Comma} Title},\n  year    = 2024,\n  month   = jan,\n  journal = {Journal},\n  note    = {100\\% sure},\n  url     = {https://example.com/%7Euser}\n}"
    );
}

#[test]
fn every_value_form_is_emitted_byte_for_byte() {
    let input = "@misc{k,\n  nested = {A {B, C} D},\n  quoted = \"Py\\TeX{} and {\\\"o}\",\n  concat = \"nob\" # \"ody\",\n  macro = tug # {ing},\n  month = jan,\n  year = 1984,\n  url = {https://ex.com/%7Euser/a_b?q=1&r=2},\n  doi = {10.1000/xyz(1)2},\n  percent = {100\\% sure and 50% off},\n}\n";
    let output = default(input);
    for value in [
        "{A {B, C} D}",
        "\"Py\\TeX{} and {\\\"o}\"",
        "\"nob\" # \"ody\"",
        "tug # {ing}",
        "jan",
        "1984",
        "{https://ex.com/%7Euser/a_b?q=1&r=2}",
        "{10.1000/xyz(1)2}",
        "{100\\% sure and 50% off}",
    ] {
        assert!(output.contains(value), "missing {value:?} in {output:?}");
    }
}

#[test]
fn paren_entries_and_braces_in_keys_are_preserved() {
    assert_eq!(
        default("@article(a1, title={T})"),
        "@article(a1,\n  title = {T}\n)"
    );
    assert_eq!(
        default("@article(k}e, title={T})"),
        "@article(k}e,\n  title = {T}\n)"
    );
}

#[test]
fn string_preamble_and_comment_blocks_keep_their_text() {
    let input = "@string{ foo = {bar} }\n@preamble{ \"\\newcommand{\\noopsort}[1]{}\" }\n@comment{@article{x, title={y}}}\n";
    assert_eq!(
        default(input),
        "@string{foo = {bar}}\n\n@preamble{\"\\newcommand{\\noopsort}[1]{}\"}\n\n@comment{@article{x, title={y}}}"
    );
    assert_eq!(default("@preamble{}"), "@preamble{}");
}

#[test]
fn comments_attach_to_the_following_block_and_trailing_comments_stay_on_their_field() {
    let input = "% top\n@misc{k,\n  title = {T}, % trailing\n  year = 2024,\n  % before close\n}\n";
    assert_eq!(
        default(input),
        "% top\n@misc{k,\n  title = {T}, % trailing\n  year  = 2024\n  % before close\n}"
    );
}

#[test]
fn preamble_and_string_concatenation_are_preserved() {
    let input =
        "@preamble{ \"prefix \" # macro # \" suffix\" }\n@string{joined = \"a\" # \"b\" # c}\n";
    let output = default(input);
    assert!(output.contains("@preamble{\"prefix \" # macro # \" suffix\"}"));
    assert!(output.contains("@string{joined = \"a\" # \"b\" # c}"));
}

#[test]
fn duplicate_fields_and_block_order_are_never_changed() {
    let input = "@misc{k,\n  year = {2000},\n  year = {2001},\n  author = {A},\n}\n@string{x = {1}}\n@misc{k,\n  title = {T},\n}";
    let output = default(input);
    assert_eq!(output.matches("year").count(), 2);
    let year2000 = output.find("2000").unwrap();
    let year2001 = output.find("2001").unwrap();
    let author = output.find("author").unwrap();
    assert!(year2000 < year2001 && year2001 < author);
    let x = output.find("@string{x = {1}}").unwrap();
    let second = output.rfind("@misc{k,").unwrap();
    assert!(x < second);
}

#[test]
fn empty_entries_and_empty_files_are_safe() {
    assert_eq!(default("@misc{}"), "@misc{}");
    assert_eq!(default("@misc{k}"), "@misc{k}");
    for input in ["", " ", "\n\n", "\t\n \n"] {
        assert_eq!(default(input), "");
    }
}

#[test]
fn malformed_bibliographies_are_rejected_without_writing() {
    for (input, message) in [
        ("@article{x, title = {unclosed", "unbalanced"),
        ("@article{x, title=\"unterminated}", "unterminated"),
        ("@article{x title={T}}", "invalid entry key"),
        ("@string{x = }", "expected a field value"),
        ("@misc{k, title = foo=bar}", "invalid bare value"),
        ("@article{x, title={T} year={2024}}", "expected ','"),
    ] {
        let error = format(input, &Config::default()).unwrap_err();
        assert!(error.to_string().contains(message), "{input}: {error}");
    }
}

#[test]
fn name_case_alignment_and_trailing_comma_are_configurable() {
    let config = Config {
        field_name_case: NameCase::Preserve,
        entry_type_case: NameCase::Preserve,
        align: false,
        trailing_comma: true,
        ..Config::default()
    };
    let output = formatted("@ARTICLE{k, Title={T}, Year=2024}", &config);
    assert_eq!(output, "@ARTICLE{k,\n  Title = {T},\n  Year = 2024,\n}");
    let config = Config {
        align_max: 3,
        ..Config::default()
    };
    assert_eq!(
        formatted("@misc{k, longname={T}, a={B}}", &config),
        "@misc{k,\n  longname = {T},\n  a = {B}\n}"
    );
}

#[test]
fn indentation_blank_lines_and_tabs_are_configurable() {
    let config = Config {
        indent: 4,
        blank_lines: 0,
        ..Config::default()
    };
    assert_eq!(
        formatted("@misc{a, title={T}}\n\n@misc{b, title={U}}", &config),
        "@misc{a,\n    title = {T}\n}\n@misc{b,\n    title = {U}\n}"
    );
    let config = Config {
        indent: 1,
        indent_type: IndentType::Tabs,
        blank_lines: 2,
        ..Config::default()
    };
    let output = formatted("@misc{a, title={T}}\n@misc{b, title={U}}", &config);
    assert!(output.contains("\n\ttitle = {T}\n"));
    assert!(output.contains("}\n\n\n@misc{b,"), "{output:?}");
}

#[test]
fn line_endings_bom_final_newline_and_crlf_are_configurable() {
    let config = Config {
        line_endings: LineEndings::Windows,
        final_newline: true,
        ..Config::default()
    };
    let output = format("\u{feff}@misc{k, title={T}}\r\n", &config).unwrap();
    assert!(output.starts_with('\u{feff}'));
    assert!(output.ends_with("\r\n"));
    assert!(!output.replace("\r\n", "").contains('\n'));

    let output = default("@misc{k, title={T}}\r\n");
    assert!(!output.ends_with('\n'));
    assert!(!output.contains('\r'));
}

#[test]
fn long_values_wrap_at_spaces_and_remain_idempotent() {
    let config = Config {
        width: 40,
        wrap: true,
        ..Config::default()
    };
    let input = "@article{k,\n  title = {A very long title that should wrap at spaces because it is longer than forty characters},\n  author = {A and B},\n}\n";
    let output = formatted(input, &config);
    assert!(
        output.lines().all(|line| line.chars().count() <= 40),
        "{output}"
    );
    assert!(output.matches("very long title").count() == 1);
    assert!(output.matches("forty characters").count() == 1);
}

#[test]
fn percent_inside_values_is_literal_and_never_starts_a_comment() {
    let output = default("@misc{k, note = {50% off}, quoted = \"100% sure\"}");
    assert!(output.contains("{50% off}"));
    assert!(output.contains("\"100% sure\""));
}

#[test]
fn string_macros_can_be_used_before_their_definition_without_reordering() {
    let input = "@misc{b, month = jan}\n@string{jan = {January}}";
    let output = default(input);
    assert!(output.find("@misc{b").unwrap() < output.find("@string{jan").unwrap());
}
