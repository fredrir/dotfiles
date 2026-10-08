use super::*;

use block::Class;
use config::Config;

fn config() -> Config {
    Config::default()
}

fn laid_out(text: &str) -> String {
    block::format(text, &config()).unwrap_or_else(|problem| {
        panic!("{}: {}", problem.line, problem.message);
    })
}

fn refused(text: &str) -> String {
    match block::format(text, &config()) {
        Ok(out) => panic!("expected a refusal, got:\n{out}"),
        Err(problem) => format!("{}: {}", problem.line, problem.message),
    }
}

fn entries(text: &str) -> Vec<(Class, String, String, String)> {
    block::signature(text).expect("the body parses")
}

// ---------------------------------------------------------------- .dotfile

#[test]
fn entries_carry_their_block_and_line_number() {
    let parsed = block::parse("host {\n  a = 1\n\n  b = 2\n}\n").expect("the body parses");
    let found: Vec<(&str, usize, &str, &str)> = parsed
        .iter()
        .filter(|line| line.class == Class::Entry)
        .map(|line| (line.block, line.number, line.key, line.value))
        .collect();

    assert_eq!(found, [("host", 2, "a", "1"), ("host", 4, "b", "2")]);
}

#[test]
fn a_comment_is_kept_rather_than_stripped() {
    // Inverted from `test_comments_are_stripped_by_default`. `blocks.scan`
    // drops comments because its callers want values; a formatter that
    // dropped them would delete the header of every file in `config/`.
    let out = laid_out("# leading\nhost {\n  a = 1 # trailing\n}\n");

    assert_eq!(out, "# leading\nhost {\n  a  = 1 # trailing\n}");
}

#[test]
fn a_trailing_comment_stays_part_of_the_value() {
    let found = entries("host {\n  a = 1 # trailing\n}\n");

    assert_eq!(found[1].3, "1 # trailing");
}

#[test]
fn an_entry_splits_on_the_first_equals_and_trims_both_sides() {
    let found = entries("group {\n  key   =   value = more\n}\n");

    assert_eq!(found[1].2, "key");
    assert_eq!(found[1].3, "value = more");
}

#[test]
fn a_line_without_an_equals_keeps_the_whole_line() {
    let found = entries("group {\n  plain\n}\n");

    assert_eq!(found[1].0, Class::Bare);
    assert_eq!((found[1].2.as_str(), found[1].3.as_str()), ("plain", ""));
}

#[test]
fn an_entry_at_top_level_is_legal() {
    // Inverted from the `OUTSIDE` case. `config/targets.dotfile` has no blocks
    // at all, so a grammar that rejected a top-level entry would reject the
    // file `dotfile link` is driven by.
    let out = laid_out("shared/starship = ~/.config\n");

    assert_eq!(out, "shared/starship = ~/.config");
}

#[test]
fn a_close_with_nothing_open_is_reported_at_its_line() {
    assert_eq!(refused("}\n"), "1: unexpected }");
}

#[test]
fn a_nested_block_left_open_is_named_in_the_diagnostic() {
    assert_eq!(refused("a {\nb {\n"), "2: missing } for b");
}

#[test]
fn a_block_left_open_is_reported_at_the_last_line() {
    assert_eq!(refused("a {\n  entry\n"), "2: missing } for a");
}

#[test]
fn a_block_left_open_is_named_in_the_diagnostic() {
    assert_eq!(refused("archie {\n  a = 1\n"), "2: missing } for archie");
}

// ------------------------------------------------------------- .dotfile layout

#[test]
fn the_equals_sits_two_columns_past_the_widest_key() {
    let out = laid_out("confmt {\nindent = 2\nalign = true\nfinal_newline = true\n}\n");

    assert_eq!(
        out,
        "confmt {\n  indent         = 2\n  align          = true\n  final_newline  = true\n}"
    );
}

#[test]
fn a_blank_line_starts_a_new_group_but_a_comment_does_not() {
    let out = laid_out("host {\n# a label\na = 1\nbb = 2\n\nlonger_key = 3\nc = 4\n}\n");

    assert_eq!(
        out,
        "host {\n  # a label\n  a   = 1\n  bb  = 2\n\n  longer_key  = 3\n  c           = 4\n}"
    );
}

#[test]
fn a_keyless_line_neither_pads_nor_widens_its_group() {
    let out = laid_out("shared {\ngit\nzsh\nnvim = neovim\n}\n");

    assert_eq!(out, "shared {\n  git\n  zsh\n  nvim  = neovim\n}");
}

#[test]
fn a_group_of_keyless_lines_alone_is_left_as_it_stands() {
    let out = laid_out("allow {\npath/to/file  label\nother/path\n}\n");

    assert_eq!(out, "allow {\n  path/to/file  label\n  other/path\n}");
}

#[test]
fn a_key_exactly_at_the_cap_still_lands_on_the_column() {
    let capped = "k".repeat(24);
    let out = laid_out(&format!("modes {{\n{capped} = a\nshort = b\n}}\n"));

    assert_eq!(
        out,
        format!(
            "modes {{\n  {capped}  = a\n  short{}= b\n}}",
            " ".repeat(21)
        )
    );
}

#[test]
fn top_level_entries_are_normalised_but_never_aligned() {
    // `add.py:targets_has_line` tests `config/targets.dotfile` for the exact
    // string `src = dst`. Pad that line and `dotfile add` appends a duplicate
    // mapping every single time it is run.
    let out = laid_out("a/very/long/source   =   ~/dest\nb = ~/other\n");

    assert_eq!(out, "a/very/long/source = ~/dest\nb = ~/other");
}

#[test]
fn an_entry_with_no_value_gets_no_trailing_space() {
    let out = laid_out("group {\nkey =\nlonger =\n}\n");

    assert_eq!(out, "group {\n  key     =\n  longer  =\n}");
}

#[test]
fn a_block_header_is_always_re_emitted_with_one_space() {
    // `blocks.py` is read with `open_suffix="{"` everywhere except
    // `packages.py`, which uses `" {"`. `name {` is the only spelling both
    // readers parse the same way.
    let out = laid_out("name{\n  a = 1\n}\n");

    assert_eq!(out, "name {\n  a  = 1\n}");
}

#[test]
fn interior_whitespace_inside_a_value_is_never_edited() {
    let out = laid_out("archie {\nMEMORY = Corsair 32 GB  (2x16 GB)   DDR5\n}\n");

    assert_eq!(
        out,
        "archie {\n  MEMORY  = Corsair 32 GB  (2x16 GB)   DDR5\n}"
    );
}

#[test]
fn blank_lines_are_dropped_at_the_edges_and_collapsed_in_the_middle() {
    let out = laid_out("\n\nhost {\n  a = 1\n\n\n\n  b = 2\n\n}\n\n\n");

    assert_eq!(out, "host {\n  a  = 1\n\n  b  = 2\n}");
}

#[test]
fn a_file_of_only_comments_keeps_them_and_adds_no_newline() {
    let out = laid_out("# one\n# two");

    assert_eq!(out, "# one\n# two");
}

#[test]
fn a_file_of_only_blank_lines_is_left_exactly_as_it_is() {
    // `format.py:format_text` truncates this to zero bytes, which is deviation
    // six: a formatter that can empty a file is one nobody can leave on save.
    assert_eq!(laid_out("\n\n\n"), "\n\n\n");
    assert_eq!(laid_out("   \n \t\n"), "   \n \t\n");
    assert_eq!(laid_out(""), "");
}

#[test]
fn a_carriage_return_does_not_survive_into_the_output() {
    let out = laid_out("host {\r\n  a = 1\r\n}\r\n");

    assert_eq!(out, "host {\n  a  = 1\n}");
}

#[test]
fn settings_can_turn_the_layout_off() {
    let config = Config {
        align: false,
        indent: 4,
        blank_lines: 0,
        final_newline: false,
        ..Config::default()
    };
    let out = block::format("host {\na = 1\n\nlonger = 2\n}\n", &config).unwrap();

    assert_eq!(out, "host {\n    a = 1\n    longer = 2\n}");
}

#[test]
fn every_tracked_dotfile_survives_a_round_trip() {
    let fixtures = [
        (
            "config/hosts.dotfile",
            include_str!("../../../../../../../../config/hosts.dotfile"),
        ),
        (
            "config/keys.dotfile",
            include_str!("../../../../../../../../config/keys.dotfile"),
        ),
        (
            "config/profiles.dotfile",
            include_str!("../../../../../../../../config/profiles.dotfile"),
        ),
        (
            "config/requirements.dotfile",
            include_str!("../../../../../../../../config/requirements.dotfile"),
        ),
        (
            "config/scan.dotfile",
            include_str!("../../../../../../../../config/scan.dotfile"),
        ),
        (
            "config/targets.dotfile",
            include_str!("../../../../../../../../config/targets.dotfile"),
        ),
    ];
    for (name, text) in fixtures {
        let out = laid_out(text);
        assert_eq!(
            entries(text),
            entries(&out),
            "{name} lost or moved an entry"
        );
        assert_eq!(out, laid_out(&out), "{name} does not settle in one pass");
    }
}

#[test]
fn nested_and_inline_empty_configuration_blocks_settle_and_keep_their_entries() {
    let input = "{\nwidth=80\n}\nconf {\nindent=2\ninclude {\n*.ssh\n}\n}\nmarkdown {}\nlua {\nwidth=120\nexcluded_files {\ninit.lua\n}\n}";
    let output = laid_out(input);
    assert_eq!(entries(input), entries(&output));
    assert_eq!(laid_out(&output), output);
    assert!(output.starts_with("{\n  width  = 80\n}"), "{output}");
    assert!(output.contains("  include {\n    *.ssh\n  }"), "{output}");
}

#[test]
fn gitignore_pattern_blocks_preserve_literal_punctuation_and_escaped_spaces() {
    let input = "lua {\nexcluded_files {\nfile=name.lua\nfile#name.lua\n\\#hash.lua\nfile{brace}.lua\ntrailing{\n\\}\nspace\\ \n}\n}\n";
    let output = laid_out(input);
    for pattern in [
        "file=name.lua",
        "file#name.lua",
        "\\#hash.lua",
        "file{brace}.lua",
        "trailing{",
        "\\}",
        "space\\ ",
    ] {
        assert!(
            output.lines().any(|line| line.trim_start() == pattern),
            "{pattern:?}: {output:?}"
        );
    }
    assert_eq!(entries(input), entries(&output));
    assert_eq!(laid_out(&output), output);
}

#[test]
fn custom_conf_extensions_preserve_ssh_assignment_syntax() {
    let output = crate::format(
        std::path::Path::new("host.ssh"),
        b"SetEnv FOO=bar  \n",
        &config(),
    )
    .unwrap();
    assert_eq!(output, "SetEnv FOO=bar");
}

#[test]
fn empty_braces_in_comments_and_values_are_preserved_as_data() {
    let input = "# comment {}\nsettings {\nkey = {}\n}\n";
    let output = laid_out(input);
    assert_eq!(output, "# comment {}\nsettings {\n  key  = {}\n}");
    assert_eq!(entries(input), entries(&output));
}

#[test]
fn inline_blocks_and_structural_comments_preserve_settings_and_comments() {
    for input in [
        "{ # global\nwidth = 80\n} # end global\nlua {} # enabled",
        "conf { include { *.ssh } }",
        "lua { width = 120 }\nmarkdown {}",
        "lua {\nquote_style = \"double\" }",
    ] {
        let output = laid_out(input);
        assert_eq!(entries(input), entries(&output), "{input}");
        assert_eq!(laid_out(&output), output, "{input}");
    }
}

#[test]
fn an_escaped_space_before_a_brace_is_part_of_a_gitignore_pattern() {
    let input = "excluded_files {\nname\\ }\n}\n";
    let output = laid_out(input);
    assert_eq!(output, "excluded_files {\n  name\\ }\n}");
    assert_eq!(entries(input), entries(&output));
}

#[test]
fn upper_case_dotfile_extensions_use_block_formatting() {
    let output = crate::format(
        std::path::Path::new("config.DOTFILE"),
        b"host {\na=1\n}",
        &config(),
    )
    .unwrap();
    assert_eq!(output, "host {\n  a  = 1\n}");
}

#[test]
fn hyphenated_file_pattern_blocks_preserve_literal_equals() {
    for name in ["included-files", "excluded-files"] {
        let input = format!("conf {{\n{name} {{\nfile=name\n}}\n}}");
        let output = laid_out(&input);
        assert!(output.contains("    file=name\n"), "{output}");
        assert_eq!(entries(&input), entries(&output));
        assert_eq!(laid_out(&output), output);
    }
}
