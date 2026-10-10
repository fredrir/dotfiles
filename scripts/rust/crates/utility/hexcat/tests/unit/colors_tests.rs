use super::*;

fn swatches(text: &str) -> Vec<Swatch> {
    let mut swatches = Vec::new();
    find(text, &mut swatches);
    swatches
}

fn colors(text: &str) -> Vec<(usize, [u8; 3])> {
    swatches(text)
        .into_iter()
        .map(|swatch| (swatch.start, swatch.rgb))
        .collect()
}

fn found(text: &str) -> Vec<[u8; 3]> {
    swatches(text)
        .into_iter()
        .map(|swatch| swatch.rgb)
        .collect()
}

#[test]
fn every_hex_length_is_a_color() {
    assert_eq!(
        found("#f80 #f80c #ff8800 #ff8800cc"),
        [[255, 136, 0], [255, 136, 0], [255, 136, 0], [255, 136, 0]]
    );
}

#[test]
fn every_hex_digit_lands_in_its_channel() {
    assert_eq!(
        found("#123456 #12345678 #abc #abcd 0x0a0b0c"),
        [
            [0x12, 0x34, 0x56],
            [0x12, 0x34, 0x56],
            [0xaa, 0xbb, 0xcc],
            [0xaa, 0xbb, 0xcc],
            [0x0a, 0x0b, 0x0c]
        ]
    );
}

#[test]
fn a_reused_buffer_holds_only_the_latest_text() {
    let mut swatches = Vec::new();
    find("#fff #000", &mut swatches);
    find("#f00", &mut swatches);
    assert_eq!(swatches, [Swatch { start: 0, rgb: [255, 0, 0] }]);
}

#[test]
fn hex_digits_are_case_insensitive() {
    assert_eq!(found("#FF8800 #Ff8800"), [[255, 136, 0], [255, 136, 0]]);
}

#[test]
fn prefixed_hex_is_a_color() {
    assert_eq!(found("0xff8800 0XFF8800"), [[255, 136, 0], [255, 136, 0]]);
}

#[test]
fn css_functions_are_colors() {
    assert_eq!(
        found("rgb(255, 136, 0) rgba(255,136,0,0.5) rgb(255 136 0 / 50%)"),
        [[255, 136, 0], [255, 136, 0], [255, 136, 0]]
    );
    assert_eq!(
        found("hsl(120, 100%, 50%) hsla(120deg 100% 50% / 0.5)"),
        [[0, 255, 0], [0, 255, 0]]
    );
}

#[test]
fn css_function_names_are_case_insensitive() {
    assert_eq!(
        found("RGB(255, 0, 0) Hsl(0, 100%, 50%)"),
        [[255, 0, 0], [255, 0, 0]]
    );
}

#[test]
fn swatches_start_at_the_code() {
    assert_eq!(
        colors(r##"a = "#ffffff", b = rgb(0, 0, 0)"##),
        [(5, [255, 255, 255]), (19, [0, 0, 0])]
    );
}

#[test]
fn swatches_from_every_form_come_out_in_order() {
    assert_eq!(
        colors("0x000000 #fff rgb(0,0,0) #000"),
        [
            (0, [0, 0, 0]),
            (9, [255, 255, 255]),
            (14, [0, 0, 0]),
            (25, [0, 0, 0])
        ]
    );
}

#[test]
fn short_digit_runs_read_as_issue_numbers() {
    assert!(found("fixes #123 and #1234 (#42)").is_empty());
}

#[test]
fn repeated_digits_and_full_codes_stay_colors() {
    assert_eq!(
        found("#333 #0000 #123456"),
        [[51, 51, 51], [0, 0, 0], [18, 52, 86]]
    );
}

#[test]
fn codes_inside_words_are_not_colors() {
    assert!(found("page.html#fff a0xffffff argb(1,2,3) hsl2(1,2,3)").is_empty());
}

#[test]
fn hex_runs_of_other_lengths_are_not_colors() {
    assert!(found("#ff #fffff #fffffff #fffffffff 0xfffff 0xffffffff").is_empty());
}

#[test]
fn unparsable_functions_are_skipped() {
    assert!(found("rgb(var(--red)) rgb(nope) hsl()").is_empty());
}

#[test]
fn functions_stop_at_the_line_end() {
    assert!(found("rgb(255,\n0, 0)").is_empty());
}
