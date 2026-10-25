#![forbid(unsafe_code)]

use dotfmt_latex::config::{Config, IndentType, LineEndings};
use dotfmt_latex::dialect::Dialect;
use dotfmt_latex::format;

fn formatted(input: &str, config: &Config) -> String {
    let output = format(input, config, Dialect::Latex).unwrap();
    assert_eq!(
        format(&output, config, Dialect::Latex).unwrap(),
        output,
        "formatting must settle after one pass"
    );
    output
}

fn default(input: &str) -> String {
    formatted(input, &Config::default())
}

#[test]
fn environments_indent_and_shared_delimiters_move_to_their_own_lines() {
    let input = "\\begin{document}\n\\begin{itemize}\n\\item one\n\\end{itemize}\n\\begin{a}\\end{a}\nafter\n\\end{document}";
    assert_eq!(
        default(input),
        "\\begin{document}\n\\begin{itemize}\n  \\item one\n\\end{itemize}\n\\begin{a}\n\\end{a}\nafter\n\\end{document}"
    );
}

#[test]
fn verbatim_blocks_and_inline_verbatim_are_byte_preserved() {
    let inner = "  spaced\ttext  \n\n\ttab line\n";
    let input = format!(
        "\\begin{{verbatim}}\n{inner}\\end{{verbatim}}\n\\verb!a%b! text\n\\lstinline|x%y|\n\\mintinline{{python}}{{print(\"hi\")}}"
    );
    let output = default(&input);
    assert!(output.contains(&format!("\\begin{{verbatim}}\n{inner}\\end{{verbatim}}")));
    assert!(output.contains("\\verb!a%b!"));
    assert!(output.contains("\\lstinline|x%y|"));
    assert!(output.contains("\\mintinline{python}{print(\"hi\")}"));
}

#[test]
fn directlua_blocks_are_preserved() {
    let block = "\\directlua{\n  local x = { 1, 2 }\n  print('%', x[1])\n}";
    let output = default(&format!("\\begin{{document}}\n{block}\n\\end{{document}}"));
    assert!(output.contains(block), "{output:?}");
}

#[test]
fn long_inline_verbatim_does_not_panic_or_wrap() {
    let minted = "\\mintinline{python}{print(\"a very long inline expression that has no pipe character anywhere at all\")}";
    let config = Config {
        width: 40,
        ..Config::default()
    };
    let output = formatted(minted, &config);
    assert_eq!(output, minted);
}

#[test]
fn commands_that_merely_start_with_begin_or_end_are_not_environments() {
    let input = "\\endcsname\\beginning\n\\begin{center}x\\end{center}\n\\bye";
    let output = default(input);
    assert!(output.contains("\\endcsname\\beginning"), "{output:?}");
    assert!(output.contains("\\begin{center}x"), "{output:?}");
    assert!(output.contains("\\end{center}"), "{output:?}");
}

#[test]
fn escaped_braces_do_not_skew_indentation() {
    let input = "\\begin{document}\n\\[\n\\left\\{ x \\right.\n\\]\nafter\n\\end{document}";
    assert_eq!(
        default(input),
        "\\begin{document}\n\\[\n  \\left\\{ x \\right.\n\\]\nafter\n\\end{document}"
    );
}

#[test]
fn a_comment_after_a_line_break_is_still_a_comment() {
    let input = "\\begin{a}\nx\\\\% comment with { braces\n\\end{a}\nafter";
    assert_eq!(
        default(input),
        "\\begin{a}\n  x\\\\% comment with { braces\n\\end{a}\nafter"
    );
}

#[test]
fn context_dialect_translates_start_and_stop_environments() {
    let input = "\\starttext\n\\startitemize[n]\n\\item first\n\\stopitemize\n\\stoptext\nafter";
    let output = format(input, &Config::default(), Dialect::Context).unwrap();
    assert_eq!(
        output,
        "\\starttext\n\\startitemize[n]\n  \\item first\n\\stopitemize\n\\stoptext\nafter"
    );
}

#[test]
fn line_endings_bom_and_final_newline_are_configurable() {
    let config = Config {
        line_endings: LineEndings::Windows,
        final_newline: true,
        wrap: false,
        ..Config::default()
    };
    let output = format(
        "\u{feff}\\begin{document}\r\nx\r\n\\end{document}\r\n",
        &config,
        Dialect::Latex,
    )
    .unwrap();
    assert!(output.starts_with('\u{feff}'));
    assert!(output.contains("\r\n"));
    assert!(output.ends_with("\r\n"));
    assert!(!output.replace("\r\n", "").contains('\n'));

    let output = default("\\begin{document}\r\nx\r\n\\end{document}\r\n");
    assert!(!output.ends_with('\n'));
}

#[test]
fn tabs_are_used_for_indentation_when_configured() {
    let config = Config {
        indent: 4,
        indent_type: IndentType::Tabs,
        wrap: false,
        ..Config::default()
    };
    let output = formatted(
        "\\begin{document}\n\\begin{itemize}\n\\item x\n\\end{itemize}\n\\end{document}",
        &config,
    );
    assert!(output.contains("\n\t\\item x\n"), "{output:?}");
}

#[test]
fn blank_line_runs_are_capped_by_configuration() {
    for (allowed, expected) in [(0, 0), (1, 1), (2, 2)] {
        let config = Config {
            blank_lines: allowed,
            wrap: false,
            ..Config::default()
        };
        let output = formatted("a\n\n\n\nb\n\n\nc\n", &config);
        let gaps: Vec<usize> = output
            .split('\n')
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|pair| pair[0].is_empty() && !pair[1].is_empty())
            .map(|_| 1)
            .collect();
        let _ = gaps;
        let mut longest = 0;
        let mut run = 0;
        for line in output.split('\n') {
            if line.is_empty() {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        assert_eq!(longest, expected, "{output:?}");
    }
}

#[test]
fn verification_accepts_a_settled_result() {
    let config = Config {
        verify: true,
        width: 50,
        ..Config::default()
    };
    let input = "\\begin{document}\n\\begin{itemize}\n\\item a line that is long enough to wrap at least once at this width\n\\end{itemize}\n\\end{document}";
    let output = formatted(input, &config);
    assert!(output.contains("\\item"));
}

#[test]
fn comments_and_code_wrap_at_the_configured_width() {
    let config = Config {
        width: 40,
        wrap: true,
        ..Config::default()
    };
    let input = "% a long comment that should wrap several times because it is long\nThis is a long line that should wrap at a space and keep going for a while.";
    let output = formatted(input, &config);
    assert!(output.lines().all(|line| line.chars().count() <= 40));
    assert!(output.contains("% "));
}

#[test]
fn tables_are_aligned_when_enabled() {
    let config = Config {
        format_tables: true,
        wrap: false,
        ..Config::default()
    };
    let input = "\\begin{tabular}{cc}\na&b\\\\\nlonger&c\\\\\n\\end{tabular}";
    let output = formatted(input, &config);
    assert!(output.contains("longer&c"), "{output}");
    assert!(
        output.lines().any(|line| line.contains("a     &b")),
        "{output}"
    );
}

#[test]
fn ignore_blocks_are_left_alone() {
    let input = "% tex-fmt: off\n  keep   this   exactly\n% tex-fmt: on\n\\begin{a}\nx\n\\end{a}";
    let output = default(input);
    assert!(output.contains("  keep   this   exactly"));
    assert!(output.contains("\\begin{a}\n  x\n\\end{a}"));
}

#[test]
fn empty_and_whitespace_only_input_becomes_empty() {
    for input in ["", " ", "\n\n", "\t\n \n"] {
        assert_eq!(default(input), "");
    }
}

#[test]
fn pathological_nesting_is_rejected_instead_of_overflowing() {
    let input = format!(
        "{}x\n{}",
        "\\begin{a}\n".repeat(120),
        "\\end{a}\n".repeat(120)
    );
    let error = format(&input, &Config::default(), Dialect::Latex).unwrap_err();
    assert!(error.to_string().contains("nesting deeper"), "{error}");
}

#[test]
fn plain_tex_and_bom_inputs_are_safe() {
    let plain = "\\beginsection One\n{\nindented text\n}\n\\bye";
    let output = format(plain, &Config::default(), Dialect::Plain).unwrap();
    assert!(output.contains("\\beginsection One"));
    assert!(output.contains("\\bye"));
    let input = "\u{feff}\\begin{document}\nx\n\\end{document}";
    assert!(default(input).starts_with('\u{feff}'));
}

#[test]
fn formatting_preserves_the_non_whitespace_token_stream() {
    let input = "\\documentclass{article}\n\\begin{document}\n\\begin{center}\\textbf{x}\\end{center}\n\\begin{itemize}\n\\item one with \\emph{two}\n\\end{itemize}\n\\[\n\\left\\{ a \\right.\n\\]\n\\begin{verbatim}\n  raw  text\n\\end{verbatim}\n\\end{document}";
    let output = default(input);
    let strip = |text: &str| {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    assert_eq!(strip(input), strip(&output));
}
