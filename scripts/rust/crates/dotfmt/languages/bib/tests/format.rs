#![forbid(unsafe_code)]

use dotfmt_bib::config::{Config, IndentType, LineEndings, NameCase};
use dotfmt_bib::format;
use dotfmt_core::diagnostic::DiagnosticKind;

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
fn paren_keys_may_contain_closing_parentheses() {
    assert_eq!(
        default("@misc(a)b, author={A. Author}, title={The Title}, year=2000)\n"),
        "@misc(a)b,\n  author = {A. Author},\n  title  = {The Title},\n  year   = 2000\n)"
    );
    assert_eq!(
        default("@misc(a))b), title={T})"),
        "@misc(a))b),\n  title = {T}\n)"
    );
    assert_eq!(default("@misc(a)b)"), "@misc(a)b)");
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
    assert_eq!(default(""), "");
}

#[test]
fn whitespace_only_input_is_preserved_instead_of_emptied() {
    for input in [" ", "\n\n", "\t\n \n"] {
        assert_eq!(default(input), input);
    }
    let config = Config {
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(formatted("\n \n", &config), "\n \n");
}

#[test]
fn whitespace_only_entry_keys_are_formatted_without_panicking() {
    assert_eq!(
        default("@misc{ }\n@misc{ok, author={A}, title={T}, year=2000}\n"),
        "@misc{}\n\n@misc{ok,\n  author = {A},\n  title  = {T},\n  year   = 2000\n}"
    );
    assert_eq!(
        default("@misc{ , title={nokey}}"),
        "@misc{,\n  title = {nokey}\n}"
    );
    let error = format("@misc{\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("unterminated entry"), "{error}");
    assert_eq!((error.line, error.column), (Some(2), Some(1)));
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
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert!(error.to_string().contains(message), "{input}: {error}");
    }
}

#[test]
fn block_types_without_an_opening_delimiter_are_diagnosed() {
    for (input, message) in [
        (
            "@article foo, title={x}",
            "expected '{' or '(' after the block type",
        ),
        ("@{k, title={x}}", "expected a block type after '@'"),
        ("@ % comment\n@misc{k}", "expected a block type after '@'"),
    ] {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert!(error.message.contains(message), "{input}: {error}");
        assert_eq!((error.line, error.column), (Some(1), Some(1)), "{input}");
    }
}

#[test]
fn unterminated_comment_blocks_are_diagnosed() {
    for input in [
        "@comment{ unbalanced\n@misc{k, title={T}}",
        "@comment(unbalanced\n@misc{k, title={T}}",
    ] {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert!(error.message.contains("unterminated @comment"), "{error}");
        assert_eq!((error.line, error.column), (Some(1), Some(1)), "{input}");
    }
}

#[test]
fn comments_between_concatenated_value_parts_are_diagnosed() {
    for input in [
        "@misc{k, title={a} # % c\n {b}}",
        "@misc{k, title = \"a\" #\n % c\n \"b\"}",
        "@string{foo = \"a\" # % c\n \"b\"}",
    ] {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert!(
            error.message.contains("between concatenated value parts"),
            "{input}: {error}"
        );
    }
}

#[test]
fn comments_before_the_end_of_a_preamble_are_diagnosed() {
    for input in [
        "@preamble{\"a\" % c\n}",
        "@preamble{ % c\n}",
        "@preamble(\"a\" % c\n)",
    ] {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert!(
            error.message.contains("before the end of @preamble"),
            "{input}: {error}"
        );
        assert_eq!(error.line, Some(1), "{input}");
    }
    assert_eq!(default("@preamble{\"a % c\"}"), "@preamble{\"a % c\"}");
    assert_eq!(default("@preamble{}"), "@preamble{}");
}

#[test]
fn trailing_comma_follows_the_setting_for_fieldless_entries_only() {
    let config = Config {
        trailing_comma: true,
        ..Config::default()
    };
    assert_eq!(formatted("@a{k}", &config), "@a{k,}");
    assert_eq!(formatted("@a(k)", &config), "@a(k,)");
    // BibTeX rejects a comma after an @string value, so it is never added.
    assert_eq!(
        formatted("@string{foo = \"x\"}", &config),
        "@string{foo = \"x\"}"
    );
    assert_eq!(
        formatted("@string{foo = \"x\"\n}\n", &config),
        "@string{foo = \"x\"}"
    );
    let config = Config::default();
    assert_eq!(formatted("@a{k,}", &config), "@a{k}");
    assert_eq!(formatted("@a{k, % kept\n}", &config), "@a{k,\n  % kept\n}");
}

#[test]
fn comments_in_a_string_block_are_placed_on_their_own_lines() {
    let config = Config {
        trailing_comma: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("@string{foo = \"x\" % c\n}", &config),
        "@string{\n  foo = \"x\"\n  % c\n}"
    );
}

#[test]
fn brace_less_comments_are_formatted_and_settle() {
    assert_eq!(
        default("@comment a note\n@misc{k, title={T}}\n"),
        "@comment a note\n\n@misc{k,\n  title = {T}\n}"
    );
    assert_eq!(
        default("@comment a note   \n@misc{k}\n"),
        "@comment a note\n\n@misc{k}"
    );
    let config = Config {
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(formatted("@comment a note", &config), "@comment a note\n");
    assert_eq!(formatted("@comment a note\n", &config), "@comment a note\n");
}

#[test]
fn wrapping_continues_after_an_over_long_token() {
    let config = Config {
        width: 30,
        wrap: true,
        ..Config::default()
    };
    let output = formatted(
        "@a{k, t={Supercalifragilisticexpialidocious aaa bbb ccc ddd eee fff ggg hhh iii}}",
        &config,
    );
    assert!(
        output.contains("  t = {Supercalifragilisticexpialidocious\n"),
        "{output}"
    );
    assert!(output.contains("eee fff"), "{output}");
    for line in output
        .lines()
        .filter(|line| !line.contains("Supercalifragilistic"))
    {
        assert!(line.chars().count() <= 30, "{line:?} in {output}");
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
