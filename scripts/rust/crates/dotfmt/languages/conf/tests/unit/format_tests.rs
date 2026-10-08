use crate::conf::{self, Mode};
use crate::config::Config;

fn config() -> Config {
    Config::default()
}

// ------------------------------------------------------------------- modes

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
    conf::format(text, Mode::Plain, config().final_newline)
}

fn hypr(text: &str) -> String {
    conf::format(text, Mode::Hypr, config().final_newline)
}

#[test]
fn plain_trims_the_edges_and_leaves_the_structure_alone() {
    let out = plain("\n\n<match target=\"font\">   \n\n\n  <edit/>  \n</match>\n\n");

    assert_eq!(out, "<match target=\"font\">\n\n  <edit/>\n</match>");
}

#[test]
fn hypr_indents_its_braces_and_normalises_its_keys() {
    let out = hypr("general{\ngaps_in=5\n  border_size   =   2\n}\n");

    assert_eq!(out, "general{\n    gaps_in = 5\n    border_size = 2\n}");
}

#[test]
fn hypr_drops_the_blank_line_above_a_closing_brace() {
    let out = hypr("animations {\n    enabled = true\n\n}\n\nmisc {\n    x = 1\n}\n");

    assert_eq!(
        out,
        "animations {\n    enabled = true\n}\n\nmisc {\n    x = 1\n}"
    );
}

#[test]
fn a_crlf_hypr_config_still_finds_its_closing_brace() {
    // Deviation 5. `rstrip(" \t")` leaves the `\r`, after which `line == "}"`
    // never matches and the file re-indents from its first brace onwards.
    let out = hypr("general {\r\ngaps_in = 5\r\n}\r\nbind = SUPER, Q\r\n");

    assert_eq!(out, "general {\n    gaps_in = 5\n}\nbind = SUPER, Q");
}

#[test]
fn final_newline_on_ends_a_conf_file_with_a_newline() {
    let out = conf::format("general{\ngaps_in=5\n}\n\n\n", Mode::Hypr, true);

    assert_eq!(out, "general{\n    gaps_in = 5\n}\n");
}

#[test]
fn final_newline_off_ends_a_conf_file_at_its_last_line() {
    let out = conf::format("general{\ngaps_in=5\n}\n\n\n", Mode::Hypr, false);

    assert_eq!(out, "general{\n    gaps_in = 5\n}");
}
