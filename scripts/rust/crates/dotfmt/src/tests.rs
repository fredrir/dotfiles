//! What each formatter does to text, and what it refuses to do to it.
//!
//! The `.dotfile` half carries ten of the thirteen tests in
//! `scripts/python/tests/core/test_blocks.py`, three of them inverted. The
//! inverted three are named for what they now assert and say why, because the
//! whole risk with an inverted test is somebody later reading it as a bug.

use super::*;

use block::Class;
use conf::Mode;
use native::Kind;
use select::{Selection, Token};

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
    let error = walk::gather(
        &absent.path().join("absent.dotfile"),
        &config::Configs::new(),
    )
    .unwrap_err();

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
    // The fixtures are the point: these eight files are what the grammar has
    // to mean, and a change here that moved an entry would be a change to data.
    let fixtures: [(&str, &str); 8] = [
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
    conf::mode(path)
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
fn no_config_can_remap_a_mode_because_the_table_is_compiled_in() {
    // The `modes` block is gone. Which files are formatted is the `include`
    // and `exclude` blocks' business; how a `.conf` file is laid out is a
    // property of the program that reads it.
    let error = settings("modes {\n  */x/*.conf = kitty\n}\n").unwrap_err();

    assert!(error.ends_with("2: unknown block: modes"), "{error}");
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

// --------------------------------------------------------------- selection

/// A selection from an `include` block and an `exclude` block, both written
/// the way a config writes them.
fn picks(include: &[&str], exclude: &[&str]) -> Selection {
    let mut selection = Selection::default();
    for entry in include {
        selection
            .include(entry)
            .unwrap_or_else(|why| panic!("{why}"));
    }
    for entry in exclude {
        selection
            .exclude(entry)
            .unwrap_or_else(|why| panic!("{why}"));
    }
    selection
}

/// Which of `paths` a selection picks up.
fn taken(selection: &Selection, paths: &[&str]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| selection.owns(Path::new(path)).is_some())
        .map(|path| (*path).to_string())
        .collect()
}

#[test]
fn only_dotfile_is_included_until_a_config_asks_for_more() {
    // The three that default off are the reason this rework happened: 49
    // tracked files here carry no extension, and picking them up because
    // somebody pointed a formatter at the tree would be a surprise.
    let selection = picks(&[], &[]);

    assert_eq!(
        taken(
            &selection,
            &[
                "a.dotfile",
                "deep/b.dotfile",
                "c.conf",
                "d.config",
                "LICENSE"
            ]
        ),
        ["a.dotfile", "deep/b.dotfile"]
    );
}

#[test]
fn a_bare_token_picks_that_extension_up_everywhere() {
    let selection = picks(&[".conf"], &[]);

    assert_eq!(
        taken(&selection, &["a.conf", "one/two/b.conf", "c.config"]),
        ["a.conf", "one/two/b.conf"]
    );
}

#[test]
fn the_empty_token_keeps_the_licence_and_the_hooks_out_until_it_is_asked_for() {
    // The exact files this repository holds, because these are the ones that
    // would be laid out by mistake.
    let names = [
        "LICENSE",
        ".githooks/pre-commit",
        "linux/kde/plasma/kdeglobals",
        "linux/arch/ssh/config.d/40-cabled",
    ];

    assert_eq!(taken(&picks(&[], &[]), &names), Vec::<String>::new());
    assert_eq!(taken(&picks(&["_empty_"], &[]), &names), names);
}

#[test]
fn a_scoped_empty_token_picks_up_the_ssh_directory_and_nothing_else() {
    // The intended usage: `**ssh` names the ssh directory, and a directory
    // holds everything below it, so the `config.d` files two levels down are
    // picked up and `LICENSE` is not.
    let selection = picks(&["**ssh/_empty_"], &[]);

    assert_eq!(
        taken(
            &selection,
            &[
                "linux/arch/ssh/config.d/40-cabled",
                "macos/ssh/config.d/42-lan",
                "shared/ssh/config",
                "LICENSE",
                ".githooks/pre-commit",
                "linux/kde/plasma/kdeglobals",
            ]
        ),
        [
            "linux/arch/ssh/config.d/40-cabled",
            "macos/ssh/config.d/42-lan",
            "shared/ssh/config"
        ]
    );
}

#[test]
fn a_later_include_entry_wins_over_an_earlier_one() {
    let selection = picks(&[".conf", "!**/kitty/.conf"], &[]);

    assert_eq!(taken(&selection, &["a.conf", "x/kitty/b.conf"]), ["a.conf"]);
}

#[test]
fn a_bang_can_take_the_built_in_dotfile_entry_away() {
    let selection = picks(&["!.dotfile"], &[]);

    assert_eq!(taken(&selection, &["a.dotfile"]), Vec::<String>::new());
}

#[test]
fn an_exclude_entry_beats_the_include_that_matched() {
    let selection = picks(&[".conf"], &["*/kitty/*"]);

    assert_eq!(taken(&selection, &["a.conf", "x/kitty/b.conf"]), ["a.conf"]);
}

#[test]
fn an_excluded_directory_takes_everything_below_it() {
    // git's rule, and the reason a `!` cannot bring one file back out of an
    // excluded directory.
    let selection = picks(&[".dotfile"], &["vendor", "!vendor/keep.dotfile"]);

    assert_eq!(
        taken(
            &selection,
            &["a.dotfile", "vendor/deep/b.dotfile", "vendor/keep.dotfile"]
        ),
        ["a.dotfile"]
    );
}

#[test]
fn a_bare_pattern_names_a_component_and_a_starred_one_is_contains() {
    let exact = picks(&["_empty_"], &["kitty"]);
    let around = picks(&["_empty_"], &["*kitty*"]);
    let paths = ["kitty/a", "mykittycat/b", "other/c"];

    assert_eq!(taken(&exact, &paths), ["mykittycat/b", "other/c"]);
    assert_eq!(taken(&around, &paths), ["other/c"]);
}

#[test]
fn a_leading_slash_anchors_to_the_directory_the_config_sits_in() {
    let anchored = picks(&["/.conf"], &[]);
    let scoped = picks(&["/deep/.conf"], &[]);

    assert_eq!(taken(&anchored, &["a.conf", "deep/b.conf"]), ["a.conf"]);
    assert_eq!(taken(&scoped, &["a.conf", "deep/b.conf"]), ["deep/b.conf"]);
}

#[test]
fn a_trailing_slash_in_an_exclude_entry_only_matches_a_directory() {
    let selection = picks(&["_empty_", ".dotfile"], &["build/"]);

    // `build/a.dotfile` goes because the directory matched; a *file* called
    // `build` stays, because the pattern asked for a directory.
    assert_eq!(
        taken(&selection, &["build/a.dotfile", "build", "b.dotfile"]),
        ["build", "b.dotfile"]
    );
}

#[test]
fn a_double_star_spans_directories_the_way_git_reads_one() {
    let selection = picks(&["one/**/.conf"], &[]);

    assert_eq!(
        taken(
            &selection,
            &["one/two/three/a.conf", "one/b.conf", "other/c.conf"]
        ),
        ["one/two/three/a.conf", "one/b.conf"]
    );
}

#[test]
fn a_double_star_spans_directories_only_when_it_stands_between_slashes() {
    // git's rule, and so gix's. `**ssh` is not "any path ending in ssh", it is
    // `*ssh`: one component ending in those three letters, which takes `.ssh`
    // and `openssh` with it. `ssh` is the spelling that means what it says.
    let loose = picks(&["**ssh/_empty_"], &[]);
    let exact = picks(&["ssh/_empty_"], &[]);
    let paths = ["a/ssh/config", "a/.ssh/config", "a/openssh/config"];

    assert_eq!(taken(&loose, &paths), paths);
    assert_eq!(taken(&exact, &paths), ["a/ssh/config"]);
}

#[test]
fn an_include_entry_that_does_not_end_in_a_token_is_refused() {
    for entry in ["*.conf", "kitty", "**ssh/", "!"] {
        let error = Selection::default().include(entry).expect_err(entry);
        assert!(error.contains("is not an include entry"), "{error}");
        assert!(error.contains("_empty_"), "{error}");
    }
}

#[test]
fn a_pattern_that_would_quietly_match_nothing_is_refused() {
    // A trailing `\` escapes a trailing space in a `.gitignore`. `block.rs`
    // has already taken the trailing whitespace off the line, so there is
    // nothing left to escape and gix answers "no match" to everything — a
    // pattern that silently does nothing at all.
    for entry in ["build\\", "one/two\\"] {
        let error = Selection::default().exclude(entry).expect_err(entry);
        assert!(error.contains("cannot end in \\"), "{error}");
    }
    let error = Selection::default()
        .include("one/two\\/.conf")
        .expect_err("one/two\\/.conf");
    assert!(error.contains("cannot end in \\"), "{error}");
}

#[test]
fn an_exclude_entry_holding_a_token_is_refused_rather_than_taken_literally() {
    // `exclude { .conf }` reads as "no .conf files" and means "no file named
    // `.conf`", which is only ever noticed by the diff it failed to prevent.
    let error = Selection::default().exclude(".conf").expect_err(".conf");

    assert_eq!(
        error,
        ".conf is an include token; an exclude entry is a plain pattern"
    );
}

#[test]
fn a_token_reads_a_name_the_way_the_formatters_do() {
    assert_eq!(Token::of(Path::new("a/b.conf")), Some(Token::Conf));
    assert_eq!(Token::of(Path::new("a/b.config")), Some(Token::Config));
    assert_eq!(Token::of(Path::new("a/b.dotfile")), Some(Token::Dotfile));
    assert_eq!(Token::of(Path::new("a/LICENSE")), Some(Token::Empty));
    // A leading dot is part of the name rather than an extension, which is
    // what `native::kind` says about it too.
    assert_eq!(Token::of(Path::new("a/.conf")), Some(Token::Empty));
    assert_eq!(Token::of(Path::new("a/b.toml")), None);
}

#[test]
fn a_config_reads_its_include_and_exclude_blocks() {
    let config = settings(
        "include {\n  .conf\n  # a comment is not a pattern\n}\n\nexclude {\n  build\n}\n",
    )
    .unwrap();
    let beside = |name: &str| config.root.join(name);

    assert_eq!(config.owns(&beside("a.conf")), Some(Token::Conf));
    assert_eq!(config.owns(&beside("build/a.conf")), None);
    assert_eq!(config.owns(&beside("a.dotfile")), Some(Token::Dotfile));
    assert_eq!(config.owns(&beside("a.toml")), None);
}

// ------------------------------------------------------------------ config

/// Write a `dotfmt.dotfile` into a fresh directory and read it back.
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
    assert_eq!(config.blank_lines, 1);
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
        ("other {\n  a = 1\n}\n", "2: unknown block: other"),
        ("indent = 4\n", "1: setting outside a block"),
        ("dotfmt {\n  indent\n}\n", "2: expected key = value"),
        ("dotfmt {\n", "1: missing } for dotfmt"),
        (
            "include {\n  a = b\n}\n",
            "2: expected a pattern; a pattern cannot hold an =",
        ),
        (
            "exclude {\n  a=b\n}\n",
            "2: expected a pattern; a pattern cannot hold an =",
        ),
        (
            "exclude {\n  build\\\n}\n",
            "2: a pattern cannot end in \\, which would escape a trailing space \
             this file no longer has: build\\",
        ),
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
fn a_config_in_a_subdirectory_beats_the_one_above_it_for_the_files_below() {
    // Resolution is per file rather than per target, so `dotfmt .` at the top
    // still reads the deeper config for the deeper files — the rule rustfmt,
    // stylua and ruff all use, and the one people expect from a config file
    // sitting next to the thing it configures.
    let root = tempfile::tempdir().unwrap();
    let deep = root.path().join("a/b");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(
        root.path().join(config::NAME),
        "dotfmt {\n  indent = 6\n}\n",
    )
    .unwrap();
    std::fs::write(deep.join(config::NAME), "dotfmt {\n  indent = 3\n}\n").unwrap();
    let configs = config::Configs::new();

    let above = configs.for_file(&root.path().join("top.dotfile")).unwrap();
    let below = configs.for_file(&deep.join("under.dotfile")).unwrap();

    assert_eq!(above.indent, 6);
    assert_eq!(below.indent, 3);
}

#[test]
fn the_chain_is_walked_once_per_directory_and_then_remembered() {
    // Walking up from every one of a few thousand files would read the same
    // three directories a few thousand times. Deleting the file the answer
    // came from is the only way to see from out here that it was not read
    // again.
    let root = tempfile::tempdir().unwrap();
    let at = root.path().join(config::NAME);
    std::fs::write(&at, "dotfmt {\n  indent = 6\n}\n").unwrap();
    let configs = config::Configs::new();

    let first = configs.for_file(&root.path().join("a.dotfile")).unwrap();
    std::fs::remove_file(&at).unwrap();
    let again = configs.for_file(&root.path().join("b.dotfile")).unwrap();

    assert_eq!(first.indent, 6);
    assert_eq!(again.indent, 6);
}

#[test]
fn the_shipped_config_lays_out_the_way_the_built_in_defaults_do() {
    // The compiled-in table and `shared/tools/dotfmt.dotfile` are two copies
    // of one decision, and the file is the one people read.
    let shipped = include_str!("../../../../../shared/tools/dotfmt.dotfile");
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
    // And it is itself laid out the way it asks for.
    assert_eq!(laid_out(shipped), shipped);
}

#[test]
fn the_shipped_config_picks_up_this_repository_and_leaves_its_scripts_alone() {
    // The three files at the top of the list are the reason `_empty_` defaults
    // off: a bash hook, a licence and a KDE settings dump, none of them
    // anything a formatter should touch. The four below it are what the
    // scoped `_empty_` entry exists for. If this test fails, look at
    // `shared/tools/dotfmt.dotfile` before looking here.
    let shipped = include_str!("../../../../../shared/tools/dotfmt.dotfile");
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(config::NAME);
    std::fs::write(&path, shipped).unwrap();
    let config = Config::read(&path).unwrap();
    let owns = |name: &str| config.owns(&root.path().join(name)).is_some();

    for left_alone in [
        "LICENSE",
        ".githooks/pre-commit",
        "linux/kde/plasma/kdeglobals",
        "macos/Brewfile",
        "environment/macos/manifest",
        "shared/ssh/bin/home-lan-connect",
    ] {
        assert!(!owns(left_alone), "{left_alone} should be left alone");
    }
    for picked_up in [
        "linux/arch/ssh/config.d/40-cabled",
        "shared/ssh/config",
        "shared/tmux/00-core.conf",
        "config/hosts.dotfile",
    ] {
        assert!(owns(picked_up), "{picked_up} should be picked up");
    }
}

// ------------------------------------------------------------------ native

#[test]
fn a_write_leaves_the_file_it_replaced_with_the_mode_it_had() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hosts.dotfile");
    std::fs::write(&path, "host {\n  a = 1\n  longer = 2\n}\n").unwrap();
    let before = std::fs::metadata(&path).unwrap().permissions();

    let outcome = native::apply(&path, "hosts.dotfile", Kind::Block, &config(), true).unwrap();

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

    let outcome = native::apply(&path, "a.dotfile", Kind::Block, &config(), true).unwrap();

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

    let outcome = native::apply(&path, "a.dotfile", Kind::Block, &config(), false).unwrap();

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

#[test]
fn a_selected_file_with_no_extension_is_laid_out_as_a_conf_file() {
    // An ssh `config.d` entry holds lines like `SetEnv FOO=bar`, which the
    // `.dotfile` formatter would read as a key and a value and write back out
    // as `SetEnv FOO = bar`. ssh does not accept that.
    assert_eq!(native::formatter(Token::Empty), Kind::Conf);
    assert_eq!(native::formatter(Token::Conf), Kind::Conf);
    assert_eq!(native::formatter(Token::Config), Kind::Conf);
    assert_eq!(native::formatter(Token::Dotfile), Kind::Block);
}

// -------------------------------------------------------------------- walk

/// Build a tree, then name every file a walk of it picks up.
fn walked(files: &[&str]) -> (tempfile::TempDir, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    for path in files {
        let at = root.path().join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "").unwrap();
    }
    let found = walk::gather(root.path(), &config::Configs::new()).unwrap();
    assert_eq!(found.problems, Vec::<String>::new());
    let named = found
        .files
        .iter()
        .map(|found| render::label(root.path(), &found.path))
        .collect();
    (root, named)
}

#[test]
fn a_walk_takes_what_the_config_includes_and_skips_the_places_nobody_formats() {
    let (_root, found) = walked(&[
        (config::NAME),
        "a.conf",
        "b.config",
        "c.dotfile",
        "d.toml",
        "deep/e.conf",
        "target/f.conf",
        "node_modules/g.conf",
        ".git/h.conf",
    ]);

    // An empty config is still a config: `.dotfile` is on, the rest are not.
    assert_eq!(found, ["c.dotfile", config::NAME]);
}

#[test]
fn a_walk_reads_the_config_of_each_directory_it_looks_in() {
    // Two subtrees, two configs, two answers. A single config resolved from
    // the target once could only ever give one of them.
    // The empty config at the top is what stops the resolution walking out of
    // the temp directory and finding the one on the machine running the test.
    let root = tempfile::tempdir().unwrap();
    for (path, body) in [
        ("dotfmt.dotfile", ""),
        ("one/dotfmt.dotfile", "include {\n  .conf\n}\n"),
        ("one/a.conf", ""),
        ("two/a.conf", ""),
    ] {
        let at = root.path().join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, body).unwrap();
    }

    let found = walk::gather(root.path(), &config::Configs::new()).unwrap();

    let named: Vec<String> = found
        .files
        .iter()
        .map(|found| render::label(root.path(), &found.path))
        .collect();
    assert_eq!(named, [config::NAME, "one/a.conf", "one/dotfmt.dotfile"]);
}

#[test]
fn a_named_file_is_used_as_given_unless_the_config_leaves_it_alone() {
    // An empty config at the top, so the one on the machine running the test
    // cannot reach in and opt `.conf` back in.
    let root = tempfile::tempdir().unwrap();
    for name in [config::NAME, "a.dotfile", "a.conf", "a.py"] {
        std::fs::write(root.path().join(name), "").unwrap();
    }
    let configs = config::Configs::new();
    let owned = root.path().join("a.dotfile");

    let found = walk::gather(&owned, &configs).unwrap();
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].path, owned);

    // A file dotfmt has no formatter for at all, and one it has a formatter
    // for and was told not to use: two situations, two answers.
    let unknown = walk::gather(&root.path().join("a.py"), &configs).unwrap_err();
    assert!(
        unknown.starts_with("not a .conf, .config or .dotfile file:"),
        "{unknown}"
    );
    let refused = walk::gather(&root.path().join("a.conf"), &configs).unwrap_err();
    assert!(
        refused.starts_with("not selected by this config:"),
        "{refused}"
    );
}

#[test]
fn a_config_that_will_not_parse_fails_its_own_directory_and_no_other() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("bad")).unwrap();
    std::fs::write(root.path().join(config::NAME), "").unwrap();
    std::fs::write(root.path().join("good.dotfile"), "").unwrap();
    std::fs::write(root.path().join("bad").join(config::NAME), "dotfmt {\n").unwrap();
    std::fs::write(root.path().join("bad/a.dotfile"), "").unwrap();

    let found = walk::gather(root.path(), &config::Configs::new()).unwrap();

    let named: Vec<String> = found
        .files
        .iter()
        .map(|found| render::label(root.path(), &found.path))
        .collect();
    assert_eq!(named, [config::NAME, "good.dotfile"]);
    assert_eq!(found.problems.len(), 1);
    assert!(
        found.problems[0].ends_with("1: missing } for dotfmt"),
        "{}",
        found.problems[0]
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

    native::apply(&link, "link.dotfile", Kind::Block, &config(), true).unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(&real).unwrap(),
        "host {\n  a       = 1\n  longer  = 2\n}\n"
    );
}
