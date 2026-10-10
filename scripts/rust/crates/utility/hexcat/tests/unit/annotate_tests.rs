use super::*;

fn annotated(lines: &[&[u8]]) -> Vec<String> {
    let mut annotator = Annotator::default();
    lines
        .iter()
        .map(|line| {
            let mut output = Vec::new();
            annotator.annotate(line, &mut output).unwrap();
            String::from_utf8_lossy(&output).into_owned()
        })
        .collect()
}

fn one(line: &str) -> String {
    annotated(&[line.as_bytes()]).remove(0)
}

fn swatch(rgb: &str, restore: &str) -> String {
    format!("\x1b[38;2;{rgb}m{SWATCH}{restore} ")
}

#[test]
fn a_swatch_goes_before_each_code() {
    assert_eq!(
        one("bg = #ff0000, fg = rgb(0, 0, 255)\n"),
        format!(
            "bg = {}#ff0000, fg = {}rgb(0, 0, 255)\n",
            swatch("255;0;0", "\x1b[39m"),
            swatch("0;0;255", "\x1b[39m")
        )
    );
}

#[test]
fn lines_without_codes_pass_through_unchanged() {
    let lines: [&[u8]; 3] = [b"plain\n", b"\x1b[31mred\x1b[0m\n", b"\xff\xfe binary\n"];
    let expected: Vec<String> = lines
        .iter()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect();
    assert_eq!(annotated(&lines), expected);
}

#[test]
fn the_active_foreground_comes_back_after_the_swatch() {
    assert_eq!(
        one("\x1b[38;2;1;2;3m\"#ffffff\"\x1b[0m\n"),
        format!(
            "\x1b[38;2;1;2;3m\"{}#ffffff\"\x1b[0m\n",
            swatch("255;255;255", "\x1b[38;2;1;2;3m")
        )
    );
}

#[test]
fn a_code_split_by_escapes_is_still_found() {
    assert_eq!(
        one("\x1b[34m#\x1b[35mff0000\x1b[0m\n"),
        format!(
            "\x1b[34m{}#\x1b[35mff0000\x1b[0m\n",
            swatch("255;0;0", "\x1b[34m")
        )
    );
}

#[test]
fn the_foreground_carries_over_to_later_lines() {
    let lines: [&[u8]; 2] = [b"\x1b[32mgreen\n", b"#000\n"];
    assert_eq!(
        annotated(&lines)[1],
        format!("{}#000\n", swatch("0;0;0", "\x1b[32m"))
    );
}

#[test]
fn a_sequence_split_across_lines_is_still_parsed() {
    let lines: [&[u8]; 2] = [b"text\x1b[3", b"3m#000\n"];
    assert_eq!(
        annotated(&lines)[1],
        format!("3m{}#000\n", swatch("0;0;0", "\x1b[33m"))
    );
}

#[test]
fn codes_after_invalid_utf8_are_found() {
    assert_eq!(
        annotated(&[b"\xff #000\n"])[0],
        format!("\u{fffd} {}#000\n", swatch("0;0;0", "\x1b[39m"))
    );
}

#[test]
fn codes_after_wide_characters_are_found() {
    assert_eq!(
        one("æøå → #000\n"),
        format!("æøå → {}#000\n", swatch("0;0;0", "\x1b[39m"))
    );
}

#[test]
fn osc_hyperlinks_do_not_hide_codes() {
    assert_eq!(
        one("\x1b]8;;https://x.y\x07#000\x1b]8;;\x07\n"),
        format!(
            "\x1b]8;;https://x.y\x07{}#000\x1b]8;;\x07\n",
            swatch("0;0;0", "\x1b[39m")
        )
    );
}

#[test]
fn a_remembered_sequence_keeps_its_effect_not_its_result() {
    let lines: [&[u8]; 2] = [b"\x1b[32m\x1b[1m#000\n", b"\x1b[33m\x1b[1m#000\n"];
    assert_eq!(
        annotated(&lines),
        [
            format!("\x1b[32m\x1b[1m{}#000\n", swatch("0;0;0", "\x1b[32m")),
            format!("\x1b[33m\x1b[1m{}#000\n", swatch("0;0;0", "\x1b[33m"))
        ]
    );
}

#[test]
fn sequences_that_are_not_colors_leave_the_foreground_alone() {
    assert_eq!(
        one("\x1b[31m\x1b[?25h\x1b[2 q#000\n"),
        format!(
            "\x1b[31m\x1b[?25h\x1b[2 q{}#000\n",
            swatch("0;0;0", "\x1b[31m")
        )
    );
}

#[test]
fn controls_inside_a_sequence_do_not_end_it() {
    assert_eq!(
        one("\x1b[3\t1m#000\n"),
        format!("\x1b[3\t1m{}#000\n", swatch("0;0;0", "\x1b[31m"))
    );
}

#[test]
fn codes_after_invalid_utf8_between_colors_are_found() {
    assert_eq!(
        annotated(&[b"\x1b[32m\xff#000\x1b[0m\n"])[0],
        format!(
            "\x1b[32m\u{fffd}{}#000\x1b[0m\n",
            swatch("0;0;0", "\x1b[32m")
        )
    );
}

#[test]
fn an_unfinished_sequence_takes_bytes_from_the_next_line() {
    let lines: [&[u8]; 2] = [b"\xe2\x82\x1b[3\n", b"hsl(0, 100%, 50%) #000\n"];
    assert_eq!(
        annotated(&lines)[1],
        format!("hsl(0, 100%, 50%) {}#000\n", swatch("0;0;0", "\x1b[39m"))
    );
}
