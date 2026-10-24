#![forbid(unsafe_code)]

use dotfmt_toml::{config::Config, format};

fn unchanged(input: &str) {
    let output = format(input, &Config::default()).unwrap();
    assert_eq!(output, input, "{input:?}");
}

#[test]
fn keeps_literals_of_every_kind() {
    unchanged(
        "hex = 0xDEAD_BEEF\noct = 0o755\nbin = 0b1010_0001\nint = -1_000_000\n\
         float = 6.626e-34\nzero = -0.0\ninf = inf\nnan = -nan\n\
         date = 1979-05-27\ntime = 07:32:00.999999\ndatetime = 1979-05-27T07:32:00-08:00\n\
         spaced = 1979-05-27 07:32:00Z\nbool = true\n",
    );
    unchanged(
        "basic = \"plain\"\nliteral = 'C:\\Users\\name'\n\
         escapes = \"tab\\tquote\\\"slash\\\\nul\\u0000\"\nescaped_key = \"key\"\n",
    );
}

#[test]
fn keeps_multi_line_strings_verbatim() {
    // Indentation inside a multi-line string is part of its value, so the
    // printer must never touch it, even where it looks misplaced.
    unchanged("text = \"\"\"\n  first\n    second  \n\"\"\"\nraw = '''\nback\\slash\n  '''\n");
    unchanged("inline = \"\"\"one\ntwo\"\"\"\n");
    unchanged("nested = [\"\"\"\nline  spaced  \n  \"\"\", 2]\n");
    // A multi-line string never gets trimmed or re-indented, but the array
    // around it is laid out.
    assert_eq!(
        format("nested = [\"\"\"a\nb\"\"\", 2]\n", &Config::default()).unwrap(),
        "nested = [\"\"\"a\nb\"\"\", 2]\n"
    );
}

#[test]
fn keeps_keys_as_written() {
    unchanged(
        "bare = 1\n\"quoted key\" = 2\n'literal key' = 3\nsite.\"google.com\" = true\n\
         a.b.c = 1\n\"dotted.header\".x = 2\n'quoted'.\"dotted\" = 3\n",
    );
    // Padding around dots is layout, so it is dropped; the spelling stays.
    assert_eq!(
        format("[ \"a.b\" . 'c' ]\nd = 1\n", &Config::default()).unwrap(),
        "[\"a.b\".'c']\nd = 1\n"
    );
    assert_eq!(
        format("'quoted' . \"dotted\" = 1\n", &Config::default()).unwrap(),
        "'quoted'.\"dotted\" = 1\n"
    );
    // A later header keeps its own quoting for a segment that an earlier
    // header wrote with different quotes.
    unchanged("[\"a b\"]\nx = 1\n[other]\ny = 2\n['a b'.child]\nz = 3\n");
    unchanged("['a b']\nx = 1\n[\"a b\".child]\ny = 2\n");
    unchanged("[[\"a b\"]]\nx = 1\n['a b'.child]\ny = 2\n");
    unchanged("[\"café\"]\n\"clé\" = \"naïve ☕\"\n");
}

#[test]
fn keeps_comments_in_every_position() {
    unchanged(
        "# leading comment\nkey = 1 # trailing comment\n# before a section\n\
         [section] # on the header\nvalue = [\n  # before an item\n  1, # on an item\n  2,\n  # before the close\n]\n\
         # end of file\n",
    );
    unchanged("a = [ # after the bracket\n]\n");
    unchanged("[[row]] # on an array of tables\nn = 1\n");
    unchanged("# only a comment\n");
    assert_eq!(
        format("# no trailing newline", &Config::default()).unwrap(),
        "# no trailing newline\n"
    );
}

#[test]
fn keeps_arrays_of_tables_in_order() {
    unchanged(
        "[[fruit]]\nname = \"apple\"\n\n[[fruit]]\nname = \"banana\"\n[fruit.physical]\ncolor = \"yellow\"\n",
    );
    unchanged("[[a.b]]\nx = 1\n[[a.b]]\nx = 2\n");
}

#[test]
fn handles_bom_and_line_endings() {
    assert_eq!(
        format(
            "\u{feff}a = 1\r\nb = \"\"\"x\r\ny\"\"\"\r\n",
            &Config::default()
        )
        .unwrap(),
        "\u{feff}a = 1\nb = \"\"\"x\ny\"\"\"\n"
    );
    let windows = Config {
        line_endings: dotfmt_toml::config::LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(
        format("\u{feff}a = 1\n", &windows).unwrap(),
        "\u{feff}a = 1\r\n"
    );
    // CRLF inside a multi-line string is a line ending, not part of the value,
    // so the windows output can be read back with either setting.
    let multi = "a = \"\"\"x\ny\"\"\"\n";
    let crlf = format(multi, &windows).unwrap();
    assert_eq!(crlf, "a = \"\"\"x\r\ny\"\"\"\r\n");
    assert_eq!(format(&crlf, &Config::default()).unwrap(), multi);
    let silent = Config {
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(
        format("a = 1\n", &silent).unwrap(),
        "a = 1",
        "final_newline = false keeps the file unterminated"
    );
}

#[test]
fn keeps_values_of_unusual_shapes() {
    unchanged("empty_array = []\nempty_inline_table = {}\n");
    unchanged("nested = [[1, 2], [{ a = 1 }]]\n");
    unchanged("inline = { a.b = 1, \"c d\" = 2 }\n");
    unchanged("mixed = [{ b = 1 }, { c = [1, 2] }]\n");
    // A dotted key keeps the comment that precedes its first component.
    unchanged("# c\n\na.b.c = 1\n");
    assert_eq!(
        format("[t]\n  # c\n  a.b = 1\n", &Config::default()).unwrap(),
        "[t]\n# c\na.b = 1\n"
    );
}
