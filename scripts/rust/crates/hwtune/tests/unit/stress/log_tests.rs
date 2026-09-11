use super::*;

#[test]
fn blocks_align_keys_without_a_final_newline() {
    let block = render_block(
        "2026-09-13T20:14:03",
        &[
            ("profile".into(), "per-core".into()),
            ("core0".into(), "pass".into()),
        ],
    );
    assert_eq!(
        block,
        "2026-09-13T20:14:03 {\n  profile  = per-core\n  core0    = pass\n}"
    );
}

#[test]
fn append_separates_blocks_with_one_blank_line() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("log.dotfile");
    append(&file, "a {\n}").unwrap();
    append(&file, "b {\n}").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "a {\n}\n\nb {\n}");
}
