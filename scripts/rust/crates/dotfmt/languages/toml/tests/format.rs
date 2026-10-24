#![forbid(unsafe_code)]

use dotfmt_toml::{config::Config, format};

fn configured(settings: &str) -> Config {
    let settings = settings
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(line, text)| {
            let (key, value) = text.split_once('=').unwrap();
            (
                key.trim().to_owned(),
                dotfmt_core::config::Setting {
                    value: value.trim().to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: line + 1,
                    global: false,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

fn stable(input: &str, expected: &str, config: &Config) {
    assert_eq!(format(input, config).unwrap(), expected, "{input:?}");
    assert_eq!(format(expected, config).unwrap(), expected, "{expected:?}");
}

#[test]
fn formats_entries_headers_and_values_idempotently() {
    stable(
        "title   =    \"TOML\"   #   note\n\n\n\n[  owner  ]\n  name = 'Tom'\n  age=42\n",
        "title = \"TOML\" #   note\n\n[owner]\nname = 'Tom'\nage = 42\n",
        &Config::default(),
    );
    stable(
        "a=1\nb =[1,2 , 3]\nc = {x=1, y = 2}\nd = [ [ 1, 2 ] ]\n",
        "a = 1\nb = [1, 2, 3]\nc = { x = 1, y = 2 }\nd = [[1, 2]]\n",
        &Config::default(),
    );
    stable(
        "[[  arr  ]]\nn = 1\n\n[[ arr ]]\nn = 2\n",
        "[[arr]]\nn = 1\n\n[[arr]]\nn = 2\n",
        &Config::default(),
    );
}

#[test]
fn compacts_entries_arrays_and_inline_tables() {
    let compact =
        configured("compact_entries = true\ncompact_arrays = true\ncompact_inline_tables = true");
    stable(
        "a = [1, 2]\nb = { x = 1, y = 2 }\n",
        "a=[1, 2]\nb={x=1, y=2}\n",
        &compact,
    );
    let padded = configured("compact_arrays = false\ncompact_inline_tables = false");
    stable(
        "a = [1, 2]\nb = { x = 1 }\nc = []\nd = {}\n",
        "a = [ 1, 2 ]\nb = { x = 1 }\nc = []\nd = {}\n",
        &padded,
    );
}

#[test]
fn expands_and_collapses_arrays_by_width() {
    let narrow = configured("width = 40");
    stable(
        "short = [1, 2, 3]\nlong = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc']\n",
        "short = [1, 2, 3]\nlong = [\n  'aaaaaaaaaa',\n  'bbbbbbbbbb',\n  'cccccccccc',\n]\n",
        &narrow,
    );
    stable("a = [\n  1,\n  2,\n]\n", "a = [1, 2]\n", &narrow);
    stable(
        "a = [[1, 2], [3, 4]]\n",
        "a = [\n  [1, 2],\n  [3, 4],\n]\n",
        &configured("width = 10"),
    );
    stable(
        "a = [\n  1,\n  2,\n]\n",
        "a = [\n  1,\n  2,\n]\n",
        &configured("array_auto_collapse = false"),
    );
    stable(
        "a = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc']\n",
        "a = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc']\n",
        &configured("width = 10\narray_auto_expand = false"),
    );
}

#[test]
fn trailing_comma_follows_the_setting() {
    stable(
        "a = [1, 2, 3, 4]\n",
        "a = [\n  1,\n  2,\n  3,\n  4,\n]\n",
        &configured("width = 15"),
    );
    stable(
        "a = [1, 2, 3, 4]\n",
        "a = [\n  1,\n  2,\n  3,\n  4\n]\n",
        &configured("width = 15\narray_trailing_comma = false"),
    );
}

#[test]
fn inline_tables_expand_only_their_values() {
    let input =
        "t = { first = ['aaaaaaaaaa', 'bbbbbbbbbb'], second = ['cccccccccc', 'dddddddddd'] }\n";
    stable(
        input,
        "t = { first = [\n  'aaaaaaaaaa',\n  'bbbbbbbbbb',\n], second = [\n  'cccccccccc',\n  'dddddddddd',\n] }\n",
        &configured("width = 60"),
    );
    stable(
        input,
        input,
        &configured("width = 60\ninline_table_expand = false"),
    );
    stable(input, input, &configured("width = 100"));
    stable(
        "a = [{ k = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc'] }]\n",
        "a = [\n  { k = [\n    'aaaaaaaaaa',\n    'bbbbbbbbbb',\n    'cccccccccc',\n  ] },\n]\n",
        &configured("width = 40"),
    );
    stable(
        "a = [{ k = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc'] }]\n",
        "a = [\n  { k = ['aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc'] },\n]\n",
        &configured("width = 40\ninline_table_expand = false"),
    );
}

#[test]
fn aligns_entries_and_comments_within_groups() {
    let input = "a = 1\nbbbb = 2\ncc = 3\n";
    stable(
        input,
        "a    = 1\nbbbb = 2\ncc   = 3\n",
        &configured("align_entries = true"),
    );
    stable(input, input, &Config::default());

    let comments = "a = 1 # one\nbbb = 22 # two\n";
    stable(
        comments,
        "a = 1    # one\nbbb = 22 # two\n",
        &Config::default(),
    );
    stable(comments, comments, &configured("align_comments = false"));

    let lone = "a = 1 # one\nlonger = 2\n";
    stable(lone, "a = 1      # one\nlonger = 2\n", &Config::default());
    stable(lone, lone, &configured("align_single_comments = false"));

    // Blank lines, comments and headers each start a new group.
    let groups = "a = 1 # one\n\nbb = 2 # two\n# split\ncc = 3 # three\n";
    stable(
        groups,
        "a = 1 # one\n\nbb = 2 # two\n# split\ncc = 3 # three\n",
        &Config::default(),
    );
}

#[test]
fn indents_tables_and_entries() {
    let input = "root = 1\n[a]\nx = 1\n[a.b]\ny = 2\n[d]\nz = 3\n";
    stable(
        input,
        "root = 1\n[a]\nx = 1\n  [a.b]\n  y = 2\n[d]\nz = 3\n",
        &configured("indent_tables = true"),
    );
    stable(
        input,
        "root = 1\n[a]\n  x = 1\n[a.b]\n  y = 2\n[d]\n  z = 3\n",
        &configured("indent_entries = true"),
    );
    stable(
        input,
        "root = 1\n[a]\n  x = 1\n  [a.b]\n    y = 2\n[d]\n  z = 3\n",
        &configured("indent_tables = true\nindent_entries = true"),
    );
    // A parent header that was printed earlier still indents a later subtable,
    // even when other sections appear in between.
    let interleaved = "[table]\na = 1\n\n[other]\nb = 2\n\n[table.child]\nc = 3\n";
    stable(
        interleaved,
        "[table]\na = 1\n\n[other]\nb = 2\n\n  [table.child]\n  c = 3\n",
        &configured("indent_tables = true"),
    );
    stable(
        interleaved,
        "[table]\n  a = 1\n\n[other]\n  b = 2\n\n  [table.child]\n    c = 3\n",
        &configured("indent_tables = true\nindent_entries = true"),
    );
    // A parent written after its child cannot indent the earlier child.
    stable(
        "[a.b.c]\nx = 1\n[a]\ny = 2\n",
        "[a.b.c]\nx = 1\n[a]\ny = 2\n",
        &configured("indent_tables = true"),
    );
    // Each array-of-tables element indents under the ancestor header printed
    // before it, so an element after [a] is indented while the first one is not.
    stable(
        "[[a.b]]\nx = 1\n[a]\ny = 2\n[[a.b]]\nz = 3\n",
        "[[a.b]]\nx = 1\n[a]\ny = 2\n  [[a.b]]\n  z = 3\n",
        &configured("indent_tables = true"),
    );
    stable(
        "[[a]]\nn = 1\n[a.b]\nx = 1\n[b]\nz = 0\n",
        "[[a]]\nn = 1\n  [a.b]\n  x = 1\n[b]\nz = 0\n",
        &configured("indent_tables = true"),
    );
}

#[test]
fn reorders_keys_within_groups() {
    let input = "b = 1 # bee\n# about a\na = 2\nc = 3\n\nz = 1\ny = 2\n[t]\nq = 1\np = 2\n";
    stable(
        input,
        "b = 1 # bee\n# about a\na = 2\nc = 3\n\ny = 2\nz = 1\n[t]\np = 2\nq = 1\n",
        &configured("reorder_keys = true"),
    );
    stable(
        "t = { b = 1, a = 2 }\n",
        "t = { a = 2, b = 1 }\n",
        &configured("reorder_keys = true"),
    );
    // The group's own-line comment stays at its head; trailing comments move
    // with the entry they belong to.
    stable(
        "# about b and a\nb = 1\na = 2\n",
        "# about b and a\na = 2\nb = 1\n",
        &configured("reorder_keys = true"),
    );
    stable(
        "c = 3 # three\nb = 2 # two\na = 1 # one\n",
        "a = 1 # one\nb = 2 # two\nc = 3 # three\n",
        &configured("reorder_keys = true"),
    );
    stable(input, input, &Config::default());
}

#[test]
fn limits_blank_lines() {
    stable(
        "a = 1\n\n\n\n\nb = 2\n",
        "a = 1\n\nb = 2\n",
        &Config::default(),
    );
    stable(
        "a = 1\n\n\n\n\nb = 2\n",
        "a = 1\nb = 2\n",
        &configured("allowed_blank_lines = 0"),
    );
    stable(
        "a = 1\n\n\n\n\nb = 2\n",
        "a = 1\n\n\n\nb = 2\n",
        &configured("allowed_blank_lines = 3"),
    );
    // A header never gains a blank line it did not have.
    stable(
        "[a]\nx = 1\n[b]\ny = 2\n",
        "[a]\nx = 1\n[b]\ny = 2\n",
        &Config::default(),
    );
}

#[test]
fn collapsing_blank_lines_keeps_alignment_groups_stable() {
    let zero = configured("allowed_blank_lines = 0");
    stable(
        "a = 1 # one\n\nbb = 22 # two\n",
        "a = 1   # one\nbb = 22 # two\n",
        &zero,
    );
    stable(
        "x = 1\n\nyy = 2\n",
        "x  = 1\nyy = 2\n",
        &configured("allowed_blank_lines = 0\nalign_entries = true"),
    );
    stable(
        "a = [\n  1, # one\n\n  22, # two\n]\n",
        "a = [\n  1,  # one\n  22, # two\n]\n",
        &zero,
    );
}

#[test]
fn applies_final_newline_and_line_endings() {
    stable("a = 1", "a = 1\n", &Config::default());
    stable("a = 1\n\n\n", "a = 1\n", &Config::default());
    let silent = Config {
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(format("a = 1\n", &silent).unwrap(), "a = 1");
    let windows = Config {
        line_endings: dotfmt_toml::config::LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(
        format("a = 1\nb = 2\n", &windows).unwrap(),
        "a = 1\r\nb = 2\r\n"
    );
    assert_eq!(format("", &Config::default()).unwrap(), "");
}

#[test]
fn keeps_dotted_keys_and_sections_in_written_order() {
    stable(
        "a.b.c = 1\nsite.\"google.com\" = true\n",
        "a.b.c = 1\nsite.\"google.com\" = true\n",
        &Config::default(),
    );
    // A parent header written after a child stays where it was written.
    stable(
        "[a.b.c]\nx = 1\n[a]\ny = 2\n",
        "[a.b.c]\nx = 1\n[a]\ny = 2\n",
        &Config::default(),
    );
    stable("[a.b]\nx = 1\n", "[a.b]\nx = 1\n", &Config::default());
    // Interleaved sections and array-of-tables elements keep their order.
    stable(
        "[a]\nx = 1\n[b]\ny = 2\n[a.c]\nz = 3\n",
        "[a]\nx = 1\n[b]\ny = 2\n[a.c]\nz = 3\n",
        &Config::default(),
    );
    stable(
        "[[a]]\nn = 1\n[b]\nx = 1\n[[a]]\nn = 2\n",
        "[[a]]\nn = 1\n[b]\nx = 1\n[[a]]\nn = 2\n",
        &Config::default(),
    );
    // Dotted-key entries keep the order they were written in.
    stable(
        "a.b = 1\nc = 2\na.d = 3\n",
        "a.b = 1\nc = 2\na.d = 3\n",
        &Config::default(),
    );
}

#[test]
fn reorder_keys_stays_within_comment_groups() {
    let config = configured("reorder_keys = true");
    stable(
        "# first\nstatic.b = 1\nstatic.a = 2\n\n# second\nstatic.d = 3\nconst.c = 4\n",
        "# first\nstatic.a = 2\nstatic.b = 1\n\n# second\nconst.c = 4\nstatic.d = 3\n",
        &config,
    );
    // A comment on its own line introduces one entry and starts a new group,
    // so these entries are never reordered.
    stable(
        "# z\nz = 1\n# y\ny = 2\n",
        "# z\nz = 1\n# y\ny = 2\n",
        &config,
    );
}

#[test]
fn formats_comments_only_when_asked() {
    let input = "a = 1 #x\nb = 2 #\nc = 3 #   spaced\n#own line\n";
    stable(input, input, &Config::default());
    stable(
        input,
        "a = 1 # x\nb = 2 #\nc = 3 #   spaced\n# own line\n",
        &configured("format_comments = true"),
    );
}
