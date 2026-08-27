//! What each formatter does to text, and what it refuses to do to it.
//!
//! The `.dotfile` half carries ten of the thirteen tests in
//! `scripts/python/tests/core/test_blocks.py`, three of them inverted. The
//! inverted three are named for what they now assert and say why, because the
//! whole risk with an inverted test is somebody later reading it as a bug.

use super::*;

use block::Class;
use conf::Mode;

fn config() -> Config {
    Config::default()
}

/// The `.dotfile` formatter, for the cases that cannot fail.
fn laid_out(text: &str) -> String {
    block::format(text, &config()).unwrap_or_else(|problem| {
        panic!("{}: {}", problem.line, problem.message);
    })
}

/// The diagnostic a body produces, as `line: message`.
fn refused(text: &str) -> String {
    match block::format(text, &config()) {
        Ok(out) => panic!("expected a refusal, got:\n{out}"),
        Err(problem) => format!("{}: {}", problem.line, problem.message),
    }
}

/// Every classified line as `(class, block, key, value)`, blanks left out —
/// what `blocks.scan` returns, and what the round-trip guard compares.
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

    assert_eq!(out, "# leading\nhost {\n  a  = 1 # trailing\n}\n");
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

    assert_eq!(out, "shared/starship = ~/.config\n");
}

#[test]
fn a_missing_target_is_a_failure_rather_than_an_empty_file() {
    // Inverted from `test_reading_a_missing_file_returns_nothing`. `[]` is the
    // right answer for a reader and the wrong one for a formatter: a typo in a
    // path would report a clean run over nothing at all.
    let absent = tempfile::tempdir().unwrap();
    let error = walk::gather(&absent.path().join("absent.dotfile")).unwrap_err();

    assert!(error.contains("absent.dotfile"), "{error}");
}

#[test]
fn a_close_with_nothing_open_is_reported_at_its_line() {
    assert_eq!(refused("}\n"), "1: unexpected }");
}

#[test]
fn a_block_inside_a_block_is_reported_at_its_line() {
    assert_eq!(refused("a {\nb {\n"), "2: nested block");
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
    let out = laid_out("dotfmt {\nindent = 2\nalign = true\nfinal_newline = true\n}\n");

    assert_eq!(
        out,
        "dotfmt {\n  indent         = 2\n  align          = true\n  final_newline  = true\n}\n"
    );
}

#[test]
fn a_blank_line_starts_a_new_group_but_a_comment_does_not() {
    let out = laid_out("host {\n# a label\na = 1\nbb = 2\n\nlonger_key = 3\nc = 4\n}\n");

    assert_eq!(
        out,
        "host {\n  # a label\n  a   = 1\n  bb  = 2\n\n  longer_key  = 3\n  c           = 4\n}\n"
    );
}

#[test]
fn a_keyless_line_neither_pads_nor_widens_its_group() {
    let out = laid_out("shared {\ngit\nzsh\nnvim = neovim\n}\n");

    assert_eq!(out, "shared {\n  git\n  zsh\n  nvim  = neovim\n}\n");
}

#[test]
fn a_group_of_keyless_lines_alone_is_left_as_it_stands() {
    let out = laid_out("allow {\npath/to/file  label\nother/path\n}\n");

    assert_eq!(out, "allow {\n  path/to/file  label\n  other/path\n}\n");
}

#[test]
fn a_key_past_the_cap_takes_one_space_and_leaves_the_group_alone() {
    // Twenty-five characters against a cap of twenty-four: it overflows into
    // its own single space rather than dragging the column out after it, and
    // `short` is laid out as though the long key were not there.
    let out = laid_out("modes {\n*/kitty/conf.d/fonts.conf = plain\nshort = hypr\n}\n");

    assert_eq!(
        out,
        "modes {\n  */kitty/conf.d/fonts.conf = plain\n  short                     = hypr\n}\n"
    );
}

#[test]
fn a_key_exactly_at_the_cap_still_lands_on_the_column() {
    let capped = "k".repeat(24);
    let out = laid_out(&format!("modes {{\n{capped} = a\nshort = b\n}}\n"));

    assert_eq!(
        out,
        format!(
            "modes {{\n  {capped}  = a\n  short{}= b\n}}\n",
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

    assert_eq!(out, "a/very/long/source = ~/dest\nb = ~/other\n");
}

#[test]
fn an_entry_with_no_value_gets_no_trailing_space() {
    let out = laid_out("group {\nkey =\nlonger =\n}\n");

    assert_eq!(out, "group {\n  key     =\n  longer  =\n}\n");
}

#[test]
fn a_block_header_is_always_re_emitted_with_one_space() {
    // `blocks.py` is read with `open_suffix="{"` everywhere except
    // `packages.py`, which uses `" {"`. `name {` is the only spelling both
    // readers parse the same way.
    let out = laid_out("name{\n  a = 1\n}\n");

    assert_eq!(out, "name {\n  a  = 1\n}\n");
}

#[test]
fn interior_whitespace_inside_a_value_is_never_edited() {
    let out = laid_out("archie {\nMEMORY = Corsair 32 GB  (2x16 GB)   DDR5\n}\n");

    assert_eq!(
        out,
        "archie {\n  MEMORY  = Corsair 32 GB  (2x16 GB)   DDR5\n}\n"
    );
}

#[test]
fn blank_lines_are_dropped_at_the_edges_and_collapsed_in_the_middle() {
    let out = laid_out("\n\nhost {\n  a = 1\n\n\n\n  b = 2\n\n}\n\n\n");

    assert_eq!(out, "host {\n  a  = 1\n\n  b  = 2\n}\n");
}

#[test]
fn a_file_of_only_comments_keeps_them_and_one_newline() {
    let out = laid_out("# one\n# two");

    assert_eq!(out, "# one\n# two\n");
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

    assert_eq!(out, "host {\n  a  = 1\n}\n");
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
    // The fixtures are the point: these nine files are what the grammar has to
    // mean, and a change here that moved an entry would be a change to data.
    let fixtures: [(&str, &str); 9] = [
        (
            "benchmarks/baselines.dotfile",
            include_str!("../../../../../benchmarks/baselines.dotfile"),
        ),
        (
            "config/hosts.dotfile",
            include_str!("../../../../../config/hosts.dotfile"),
        ),
        (
            "config/keys.dotfile",
            include_str!("../../../../../config/keys.dotfile"),
        ),
        (
            "config/packages.dotfile",
            include_str!("../../../../../config/packages.dotfile"),
        ),
        (
            "config/pins.dotfile",
            include_str!("../../../../../config/pins.dotfile"),
        ),
        (
            "config/profiles.dotfile",
            include_str!("../../../../../config/profiles.dotfile"),
        ),
        (
            "config/requirements.dotfile",
            include_str!("../../../../../config/requirements.dotfile"),
        ),
        (
            "config/scan.dotfile",
            include_str!("../../../../../config/scan.dotfile"),
        ),
        (
            "config/targets.dotfile",
            include_str!("../../../../../config/targets.dotfile"),
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

// ------------------------------------------------------------------- modes

/// The mode a path is formatted in under the compiled-in patterns.
fn mode_of(path: &str) -> Mode {
    config().mode_for(path)
}

#[test]
fn each_built_in_pattern_picks_its_mode() {
    assert_eq!(mode_of("/home/x/.config/hypr/hyprland.conf"), Mode::Hypr);
    assert_eq!(mode_of("/home/x/.config/hypr-local.conf"), Mode::Hypr);
    assert_eq!(mode_of("hyprland.conf"), Mode::Hypr);
    assert_eq!(
        mode_of("/home/x/.config/kitty/colors-mocha.conf"),
        Mode::Plain
    );
    assert_eq!(mode_of("/home/x/.config/colors.conf"), Mode::Plain);
    assert_eq!(mode_of("/home/x/kitty/conf.d/fonts.conf"), Mode::Plain);
    assert_eq!(mode_of("/home/x/.config/kitty/tabs.conf"), Mode::Kitty);
    assert_eq!(mode_of("/home/x/.config/kitty.conf"), Mode::Kitty);
    assert_eq!(mode_of("shared/tmux/00-core.conf"), Mode::Plain);
}

#[test]
fn a_plain_pattern_beats_the_kitty_pattern_it_sits_inside() {
    // `*/kitty/colors*.conf` and `*/kitty/*.conf` both match a colour scheme,
    // and only one of them should. The `plain` opt-out is listed first, and
    // the first match is the one that wins.
    assert_eq!(mode_of("~/.config/kitty/colors-mocha.conf"), Mode::Plain);
    assert_eq!(mode_of("~/.config/kitty/conf.d/fonts.conf"), Mode::Plain);
}

#[test]
fn a_star_crosses_a_slash_the_way_fnmatch_lets_it() {
    // The reason this matcher is hand-written. Every glob crate stops `*` at a
    // separator, which would quietly stop matching the files there are.
    assert!(conf::matches("*/kitty/*.conf", "/a/b/kitty/conf.d/x.conf"));
    assert!(conf::matches(
        "*/hypr/*",
        "/home/x/.config/hypr/conf.d/rules.conf"
    ));
    assert!(!conf::matches("*/kitty/*.conf", "/a/b/kitty/conf.d/x.ini"));
}

#[test]
fn the_matcher_handles_the_rest_of_what_fnmatch_reads() {
    assert!(conf::matches("*", ""));
    assert!(conf::matches("**/x", "a/b/x"));
    assert!(conf::matches("a?c", "abc"));
    assert!(!conf::matches("a?c", "ac"));
    assert!(conf::matches("[abc]at.conf", "bat.conf"));
    assert!(!conf::matches("[abc]at.conf", "dat.conf"));
    assert!(conf::matches("[!abc]at.conf", "dat.conf"));
    assert!(conf::matches("[a-z]at.conf", "hat.conf"));
    assert!(!conf::matches("[a-z]at.conf", "Hat.conf"));
    // An unclosed bracket is a literal bracket, as it is in Python.
    assert!(conf::matches("[abc.conf", "[abc.conf"));
    assert!(conf::matches("colors*.conf", "colors.conf"));
    assert!(!conf::matches("colors*.conf", "color.conf"));
}

// ----------------------------------------------------------------- .conf

fn plain(text: &str) -> String {
    conf::format(text, Mode::Plain)
}

fn hypr(text: &str) -> String {
    conf::format(text, Mode::Hypr)
}

fn kitty(text: &str) -> String {
    conf::format(text, Mode::Kitty)
}

#[test]
fn plain_trims_the_edges_and_leaves_the_structure_alone() {
    let out = plain("\n\n<match target=\"font\">   \n\n\n  <edit/>  \n</match>\n\n");

    assert_eq!(out, "<match target=\"font\">\n\n  <edit/>\n</match>\n");
}

#[test]
fn hypr_indents_its_braces_and_normalises_its_keys() {
    let out = hypr("general{\ngaps_in=5\n  border_size   =   2\n}\n");

    assert_eq!(out, "general{\n    gaps_in = 5\n    border_size = 2\n}\n");
}

#[test]
fn hypr_leaves_a_left_hand_side_that_is_not_a_key_alone() {
    let out = hypr("bind = SUPER, Q, exec, kitty\nnot a key = value\n");

    assert_eq!(out, "bind = SUPER, Q, exec, kitty\nnot a key = value\n");
}

#[test]
fn hypr_drops_the_blank_line_above_a_closing_brace() {
    let out = hypr("animations {\n    enabled = true\n\n}\n\nmisc {\n    x = 1\n}\n");

    assert_eq!(
        out,
        "animations {\n    enabled = true\n}\n\nmisc {\n    x = 1\n}\n"
    );
}

#[test]
fn a_brace_inside_a_value_does_not_open_a_block() {
    // Deviation 4. `format.py` opens a block on any non-comment line ending in
    // `{`, so this rule used to re-indent every line after it to end of file.
    let out = hypr("windowrulev2 = float,class:^(x){\nbind = SUPER, Q, exec, kitty\n");

    assert_eq!(
        out,
        "windowrulev2 = float,class:^(x){\nbind = SUPER, Q, exec, kitty\n"
    );
}

#[test]
fn a_brace_in_a_comment_does_not_open_a_block() {
    let out = hypr("# a note about {\nbind = SUPER, Q, exec, kitty\n");

    assert_eq!(out, "# a note about {\nbind = SUPER, Q, exec, kitty\n");
}

#[test]
fn a_crlf_hypr_config_still_finds_its_closing_brace() {
    // Deviation 5. `rstrip(" \t")` leaves the `\r`, after which `line == "}"`
    // never matches and the file re-indents from its first brace onwards.
    let out = hypr("general {\r\ngaps_in = 5\r\n}\r\nbind = SUPER, Q\r\n");

    assert_eq!(out, "general {\n    gaps_in = 5\n}\nbind = SUPER, Q\n");
}

#[test]
fn a_conf_file_of_only_whitespace_is_left_exactly_as_it_is() {
    // Deviation 6.
    assert_eq!(plain("\n \n\t\n"), "\n \n\t\n");
    assert_eq!(hypr("\n\n"), "\n\n");
    assert_eq!(kitty("\n\n"), "\n\n");
    assert_eq!(plain(""), "");
}

#[test]
fn kitty_lays_out_its_two_columns_independently() {
    let out =
        kitty("font_family   Fira Code\nfont_size 12\nmap ctrl+shift+t new_tab\nmap f1 launch\n");

    assert_eq!(
        out,
        "font_family  Fira Code\nfont_size    12\nmap ctrl+shift+t  new_tab\nmap f1            launch\n"
    );
}

#[test]
fn kitty_keeps_what_is_inside_a_quote_exactly_as_it_was() {
    let out = kitty("a 1\nfoo \"two   words\"   tail\n");

    assert_eq!(out, "a    1\nfoo  \"two   words\" tail\n");
}

#[test]
fn an_apostrophe_in_a_comment_does_not_swallow_the_line() {
    // Deviation 3. `format.py` compacts before it asks about `#`, so the `'`
    // here opens a quote that never closes and the spacing after it survives
    // into a line nobody meant to keep.
    let out = kitty("# don't   collapse   this\nfont_size 12\n");

    assert_eq!(out, "# don't   collapse   this\nfont_size  12\n");
}

#[test]
fn kitty_keeps_a_map_line_that_has_no_action_as_it_found_it() {
    let out = kitty("map f1\nfont_size 12\n");

    assert_eq!(out, "map f1\nfont_size  12\n");
}

// ------------------------------------------------------------------ config

/// Write a `dotfile.dotfile` into a fresh directory and read it back.
fn settings(body: &str) -> Result<Config, String> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(config::NAME);
    std::fs::write(&path, body).unwrap();
    Config::read(&path)
}

#[test]
fn a_config_file_overrides_only_what_it_names() {
    let config = settings("dotfmt {\n  indent = 4\n}\n").unwrap();

    assert_eq!(config.indent, 4);
    assert_eq!(config.align_max, 24);
    assert_eq!(config.modes.len(), 8);
}

#[test]
fn a_modes_block_replaces_the_built_in_patterns() {
    let config = settings("modes {\n  */x/*.conf = kitty\n}\n").unwrap();

    assert_eq!(config.mode_for("/a/x/b.conf"), Mode::Kitty);
    assert_eq!(config.mode_for("/a/hypr/b.conf"), Mode::Plain);
}

#[test]
fn a_mistake_in_the_config_is_reported_at_its_line() {
    let faults = [
        ("dotfmt {\n  indnet = 4\n}\n", "2: unknown setting: indnet"),
        (
            "dotfmt {\n  indent = wide\n}\n",
            "2: indent must be a whole number, not wide",
        ),
        (
            "dotfmt {\n  align = maybe\n}\n",
            "2: align must be true or false, not maybe",
        ),
        ("modes {\n  */x/* = shouty\n}\n", "2: unknown mode: shouty"),
        ("other {\n  a = 1\n}\n", "2: unknown block: other"),
        ("indent = 4\n", "1: setting outside a block"),
        ("dotfmt {\n  indent\n}\n", "2: expected key = value"),
        ("dotfmt {\n", "1: missing } for dotfmt"),
    ];
    for (body, expected) in faults {
        let error = settings(body).expect_err(body);
        assert!(error.ends_with(expected), "{error} should end {expected}");
        assert!(error.contains(config::NAME), "{error} should name the file");
    }
}

#[test]
fn the_nearest_config_above_the_target_is_the_one_that_governs() {
    let root = tempfile::tempdir().unwrap();
    let deep = root.path().join("a/b");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(
        root.path().join(config::NAME),
        "dotfmt {\n  indent = 6\n}\n",
    )
    .unwrap();

    assert_eq!(Config::resolve(&deep).unwrap().indent, 6);
    assert_eq!(Config::resolve(root.path()).unwrap().indent, 6);
}

#[test]
fn the_shipped_config_says_what_the_built_in_defaults_say() {
    // The compiled-in table and `shared/tools/dotfile.dotfile` are two copies
    // of one decision, and the file is the one people read.
    let shipped = include_str!("../../../../../shared/tools/dotfile.dotfile");
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(config::NAME);
    std::fs::write(&path, shipped).unwrap();
    let config = Config::read(&path).unwrap();
    let built_in = Config::default();

    assert_eq!(config.indent, built_in.indent);
    assert_eq!(config.align, built_in.align);
    assert_eq!(config.align_max, built_in.align_max);
    assert_eq!(config.blank_lines, built_in.blank_lines);
    assert_eq!(config.final_newline, built_in.final_newline);
    assert_eq!(config.modes, built_in.modes);
    // And it is itself laid out the way it asks for.
    assert_eq!(laid_out(shipped), shipped);
}

// ------------------------------------------------------------------ native

#[test]
fn a_write_leaves_the_file_it_replaced_with_the_mode_it_had() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hosts.dotfile");
    std::fs::write(&path, "host {\n  a = 1\n  longer = 2\n}\n").unwrap();
    let before = std::fs::metadata(&path).unwrap().permissions();

    let outcome = native::apply(&path, "hosts.dotfile", &config(), true).unwrap();

    assert_eq!(outcome.done, native::Done::Changed);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "host {\n  a       = 1\n  longer  = 2\n}\n"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().permissions(), before);
    // The temp file is a sibling, and it does not survive the rename.
    let left: Vec<String> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().display().to_string())
        .collect();
    assert_eq!(left, ["hosts.dotfile"]);
}

#[test]
fn a_file_already_formatted_is_not_written_again() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.dotfile");
    std::fs::write(&path, "host {\n  a  = 1\n}\n").unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    let outcome = native::apply(&path, "a.dotfile", &config(), true).unwrap();

    assert_eq!(outcome.done, native::Done::Unchanged);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn check_works_out_the_answer_without_touching_the_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.dotfile");
    std::fs::write(&path, "host {\n  a = 1\n  longer = 2\n}\n").unwrap();

    let outcome = native::apply(&path, "a.dotfile", &config(), false).unwrap();

    assert_eq!(outcome.done, native::Done::Changed);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "host {\n  a = 1\n  longer = 2\n}\n"
    );
}

#[test]
fn a_file_dotfmt_does_not_own_comes_back_untouched() {
    let text = "x  =  1\n\n\n";
    let out = native::format(Path::new("a.py"), "a.py", text, &config()).unwrap();

    assert_eq!(out, text);
}

#[test]
fn a_structural_failure_names_the_file_and_the_line() {
    let error = native::format(
        Path::new("config/hosts.dotfile"),
        "config/hosts.dotfile",
        "a {\nb {\n}\n",
        &config(),
    )
    .unwrap_err();

    assert_eq!(error, "config/hosts.dotfile:2: nested block");
}

#[test]
fn the_extension_decides_which_formatter_owns_a_file() {
    assert_eq!(
        native::kind(Path::new("a/b.conf")),
        Some(native::Kind::Conf)
    );
    assert_eq!(
        native::kind(Path::new("a/b.config")),
        Some(native::Kind::Conf)
    );
    assert_eq!(
        native::kind(Path::new("a/b.dotfile")),
        Some(native::Kind::Block)
    );
    assert_eq!(native::kind(Path::new("a/b.toml")), None);
    assert_eq!(native::kind(Path::new("a/b")), None);
}

// -------------------------------------------------------------------- walk

#[test]
fn a_walk_finds_the_three_extensions_and_skips_the_places_nobody_formats() {
    let root = tempfile::tempdir().unwrap();
    for path in [
        "a.conf",
        "b.config",
        "c.dotfile",
        "d.toml",
        "deep/e.conf",
        "target/f.conf",
        "node_modules/g.conf",
        ".git/h.conf",
    ] {
        let at = root.path().join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "").unwrap();
    }
    let found: Vec<String> = walk::gather(root.path())
        .unwrap()
        .iter()
        .map(|path| render::label(root.path(), path))
        .collect();

    assert_eq!(found, ["a.conf", "b.config", "c.dotfile", "deep/e.conf"]);
}

#[test]
fn a_named_file_is_used_as_given_unless_dotfmt_does_not_own_it() {
    let root = tempfile::tempdir().unwrap();
    let owned = root.path().join("a.conf");
    let other = root.path().join("a.py");
    std::fs::write(&owned, "").unwrap();
    std::fs::write(&other, "").unwrap();

    assert_eq!(walk::gather(&owned).unwrap(), std::slice::from_ref(&owned));
    let error = walk::gather(&other).unwrap_err();
    assert!(
        error.starts_with("not a .conf, .config or .dotfile file:"),
        "{error}"
    );
}

// ------------------------------------------------------------------ labels

#[test]
fn a_label_is_relative_to_the_root_the_run_was_pointed_at() {
    assert_eq!(
        render::label(Path::new("."), Path::new("./config/hosts.dotfile")),
        "config/hosts.dotfile"
    );
    assert_eq!(
        render::label(Path::new("/a/b"), Path::new("/a/b/c/d.conf")),
        "c/d.conf"
    );
}

#[test]
fn a_file_reached_through_a_symlink_is_written_through_it() {
    // Half the configs this repository owns are reached as a link in
    // `~/.config`. Renaming over the link would replace it with a regular file
    // and leave the copy under version control unformatted.
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("repo/hosts.dotfile");
    let link = root.path().join("link.dotfile");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "host {\n  a = 1\n  longer = 2\n}\n").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    native::apply(&link, "link.dotfile", &config(), true).unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(&real).unwrap(),
        "host {\n  a       = 1\n  longer  = 2\n}\n"
    );
}
