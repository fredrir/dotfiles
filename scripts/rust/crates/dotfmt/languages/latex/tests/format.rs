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
        "\\begin{document}\n\\begin{itemize}\n  \\item one\n\\end{itemize}\n\\begin{a}%\n\\end{a}\nafter\n\\end{document}"
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
fn lua_blocks_are_preserved() {
    let directlua =
        "\\directlua{\nlocal x = { 1, 2 }\nif x then\nprint('%', [[a   b\n  c]])\nend\n}";
    let luaexec = "\\luaexec{\nfor i = 1, 3 do\ntex.print(i)\nend\n}";
    let luadirect = "\\luadirect{\nlocal y = 1\n}";
    let input =
        format!("\\begin{{document}}\n{directlua}\n{luaexec}\n{luadirect}\n\\end{{document}}");
    let output = default(&input);
    for block in [directlua, luaexec, luadirect] {
        assert!(output.contains(block), "{output:?}");
    }
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
    // The split guard writes a line-final `%` so the inserted newline is not a
    // space token; it is part of the layout, not of the token stream.
    let strip = |text: &str| {
        text.replace("%\n", "\n")
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    assert_eq!(strip(input), strip(&output));
}

#[test]
fn display_math_is_translated_only_when_each_delimiter_starts_a_line() {
    let input = "\\catcode`\\[12\\catcode`\\]12\n\\[\nx\n\\]";
    let output = default(input);
    assert!(
        output.contains("\\catcode`\\[12\\catcode`\\]12"),
        "{output:?}"
    );
    assert!(!output.contains("`\n"), "{output:?}");
    assert!(output.contains("\\[\n  x\n\\]"), "{output:?}");
    assert_eq!(default("a \\[ x \\] b"), "a \\[ x \\] b");
    assert_eq!(default("\\[ x \\] b"), "\\[ x \\] b");
}

#[test]
fn a_guarded_split_leaves_the_token_stream_unchanged() {
    let output = default("\\def\\foo#1\\end{bar}");
    assert_eq!(output, "\\def\\foo#1%\n\\end{bar}");
    let output = default("[Hello\\begin{ital}world\\end{ital}]");
    assert_eq!(output, "[Hello%\n\\begin{ital}world%\n\\end{ital}]");
    assert_eq!(default("a \\begin{x}"), "a\n\\begin{x}");
}

#[test]
fn mid_line_item_and_sectioning_commands_are_guarded() {
    assert_eq!(default("a\\item b"), "a%\n\\item b");
    assert_eq!(default("x\\section{y}"), "x%\n\\section{y}");
    assert_eq!(default("x \\subsection*{y}"), "x\n\\subsection*{y}");
    assert_eq!(default("x\\itemize"), "x\\itemize");
    assert_eq!(default("x\\partial"), "x\\partial");
}

#[test]
fn a_bare_path_does_not_protect_the_rest_of_the_line() {
    let input = "{\\begin{tikzpicture}\n\\path; }{\\end{tikzpicture}}\nZ";
    let output = default(input);
    assert!(output.contains("\\path; }"), "{output:?}");
    assert!(output.ends_with("\nZ"), "{output:?}");
}

#[test]
fn non_ascii_whitespace_is_preserved() {
    let input = "\\newcommand{\\f}[1]{\n\u{a0}\t\\renewcommand{\\a}{#1}\n\u{a0}\t\\renewcommand{\\b}{#1}\n}";
    let output = default(input);
    assert_eq!(output.matches('\u{a0}').count(), 2, "{output:?}");
    assert_eq!(default("a\u{2003}b"), "a\u{2003}b");
}

#[test]
fn escaped_splitter_text_stays_on_one_line() {
    let output = default("\\begin{a}\nx\\\\item y\n\\end{a}");
    assert!(output.contains("x\\\\item y"), "{output:?}");
    assert!(!output.contains("x\\\n"), "{output:?}");
    let output = default("x\\\\begin{a} y");
    assert!(output.contains("x\\\\begin{a} y"), "{output:?}");
}

#[test]
fn off_regions_are_preserved_byte_for_byte() {
    let region = "% tex-fmt: off\n\\foo   \\begin{verbatim}\n\t raw\t x   \n\n\n\\end{verbatim}\n% tex-fmt: on";
    let input = format!("\\def\\m{{a\\begin{{tabular}}{{c}}b\\end{{tabular}}c}}\n{region}\nafter");
    let output = default(&input);
    assert!(output.contains(region), "{output:?}");
    assert!(output.contains("\\def\\m{a%\n"), "{output:?}");
}

#[test]
fn nested_protection_tokens_are_restored() {
    let input =
        "\\begin{verbatim}\n% tex-fmt: off\nraw   text\n% tex-fmt: on\n\\end{verbatim}\nafter";
    let output = default(input);
    assert!(
        output.contains(
            "\\begin{verbatim}\n% tex-fmt: off\nraw   text\n% tex-fmt: on\n\\end{verbatim}"
        ),
        "{output:?}"
    );

    let input = "a\\\\item \\verb|x|";
    let output = default(input);
    assert!(output.contains("\\\\item \\verb|x|"), "{output:?}");
}

#[test]
fn off_markers_inside_verbatim_text_are_not_directives() {
    let input = "\\begin{verbatim}\n% tex-fmt: off\nraw   text\n\\end{verbatim}\n   after   text";
    let output = default(input);
    assert!(
        output.contains("\\begin{verbatim}\n% tex-fmt: off\nraw   text\n\\end{verbatim}"),
        "{output:?}"
    );
    assert!(!output.contains("   after"), "{output:?}");
}

#[test]
fn a_frozen_split_line_keeps_the_following_environment_visible() {
    let input = "\\begin{a}\nx\\\\item y\\end{a}\nafter";
    let output = default(input);
    assert!(output.contains("\\end{a}\nafter"), "{output:?}");
}

#[test]
fn blank_line_markers_are_never_wrapped_into_the_output() {
    let config = Config {
        width: 10,
        ..Config::default()
    };
    let input = "\\begin{document}\n\\begin{itemize}\n\\item a\n\n\\item b\n\\end{itemize}\n\\end{document}";
    let output = formatted(input, &config);
    assert!(!output.contains('\u{e000}'), "{output:?}");
    assert!(output.contains("\\item a\n\n"), "{output:?}");
}

#[test]
fn tooling_directives_are_never_rewrapped() {
    let config = Config {
        width: 30,
        wrap: true,
        ..Config::default()
    };
    let input = "%!TEX program = pdflatex, shell-escape, synctex=1, cjk-escapes\n%!TEX root = /home/user/a/very/long/path/to/the/main/document.tex\n% !BIB program = biber and some more options here\n% arara: pdflatex: { options: [ \"-shell-escape\", \"--interaction=nonstopmode\" ] }\n%&format=pdflatex";
    assert_eq!(formatted(input, &config), input);
    let wrapped = formatted(
        "% an ordinary comment that is long enough to wrap at this width",
        &config,
    );
    assert!(wrapped.contains('\n'), "{wrapped:?}");
}

#[test]
fn private_use_characters_in_the_input_are_not_token_markers() {
    let input =
        "\u{e000}\u{e001}\u{e002}\u{e003}\u{e004}\u{e005}\u{e006}\u{e007}\u{f0000} \\verb|xyz|";
    assert_eq!(default(input), input);
}

#[test]
fn an_input_that_uses_the_whole_private_use_area_is_rejected() {
    let input: String = (0xe000..=0xf8ff)
        .map(|code| char::from_u32(code).expect("private-use scalar value"))
        .collect();
    let error = format(&input, &Config::default(), Dialect::Latex).unwrap_err();
    assert!(error.to_string().contains("private-use"), "{error}");
}

#[test]
fn table_alignment_does_not_panic_on_a_multibyte_row_start() {
    let config = Config {
        format_tables: true,
        wrap: false,
        ..Config::default()
    };
    // A leading `(` is protected as a private-use token, which is multi-byte
    // for tex-fmt as well.
    for input in [
        "\\begin{tabular}{cc}\n\u{2603} & b\\\\ c\nx & y\\\\\n\\end{tabular}",
        "\\begin{tabular}{cc}\n(a) & b\\\\ c\nx & y\\\\\n\\end{tabular}",
        // The panic also fires on a row produced by an earlier `\\ ` split.
        "\\begin{tabular}{cc}\na & b\\\\(c) & d\\\\ e\n\\end{tabular}",
        "\\begin{tabular}{cc}\na & b\\\\ \u{2603} c\\\\ d\n\\end{tabular}",
    ] {
        let output = formatted(input, &config);
        let strip = |text: &str| {
            text.chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(strip(input), strip(&output));
    }
}

#[test]
fn verbatim_like_environments_are_byte_preserved() {
    let body = "   keep   me\n\ttab and  spaces\na line that is far longer than the configured width and must not be wrapped at all";
    for name in [
        "alltt",
        "alltt*",
        "semiverbatim",
        "SaveVerbatim",
        "BVerbatim*",
        "LVerbatim*",
        "verbatimtab",
    ] {
        let block = format!("\\begin{{{name}}}\n{body}\n\\end{{{name}}}");
        let output = default(&block);
        assert!(output.contains(&block), "{name}: {output:?}");
    }
}
