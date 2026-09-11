use super::*;

#[test]
fn strict_ansi_uses_basic_codes_across_split_writes() {
    let mut writer = SgrWriter::with_depth(Vec::new(), ColorDepth::Ansi16);
    for bytes in [b"text\x1b[38;".as_slice(), b"5;9;48;5;4mhello\x1b[0m"] {
        writer.write_all(bytes).unwrap();
    }
    writer.flush().unwrap();
    assert_eq!(writer.get_ref(), b"text\x1b[91;44mhello\x1b[0m");
}

#[test]
fn strict_ansi_reduces_effect_rgb_and_passes_other_controls() {
    let mut writer = SgrWriter::with_depth(Vec::new(), ColorDepth::Ansi16);
    writer
        .write_all(b"\x1b[38;2;255;0;0mred\x1b[2J\x1b[?25l")
        .unwrap();
    writer.flush().unwrap();
    assert_eq!(writer.get_ref(), b"\x1b[91mred\x1b[2J\x1b[?25l");
}

#[test]
fn other_color_depths_preserve_bytes_and_long_sequences_stay_bounded() {
    let input = b"\x1b[38;5;9mred";
    let mut direct = SgrWriter::with_depth(Vec::new(), ColorDepth::Ansi256);
    direct.write_all(input).unwrap();
    assert_eq!(direct.get_ref(), input);
    let mut strict = SgrWriter::with_depth(Vec::new(), ColorDepth::Ansi16);
    let long = format!("\x1b[{}m", "1;".repeat(100));
    strict.write_all(long.as_bytes()).unwrap();
    assert!(strict.pending.len() < MAX_SEQUENCE);
    strict.flush().unwrap();
    assert_eq!(strict.get_ref(), long.as_bytes());
}
