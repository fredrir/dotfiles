#![forbid(unsafe_code)]

use dotfmt_yaml::{config::Config, format};

fn configured(settings: &str) -> Config {
    let settings = settings
        .lines()
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
fn formats_mappings_sequences_and_flow_collections_idempotently() {
    stable(
        "a:   1\nb:\n- x\n-   y\nc: {d: 1,   e: [1,2]}\n\n\n\nf:   g   # note\n",
        "a: 1\nb:\n  - x\n  - y\nc: {d: 1, e: [1, 2]}\n\nf: g # note\n",
        &Config::default(),
    );
    stable(
        "a:\n    b:\n        - 1\n        - x: y\n          z: w\n",
        "a:\n  b:\n    - 1\n    - x: y\n      z: w\n",
        &Config::default(),
    );
    stable(
        "-   a: 1\n    b: 2\n-\n  c: 3\n- - x\n  - y\n",
        "- a: 1\n  b: 2\n- c: 3\n- - x\n  - y\n",
        &Config::default(),
    );
}

#[test]
fn preserves_comments_anchors_tags_documents_and_explicit_keys() {
    for input in [
        "# top\nbase: &base\n  x: 1\nderived:\n  <<: *base\n  y: !!str 2\n*base : aliased\n",
        "%YAML 1.2\n---\na: 1\n...\n--- # second\nb: 2\n---\n- x\n",
        "a: !custom\n  b: 1\nc: !<tag:yaml.org,2002:str> x\nd: !!set\n  ? x\n  ? y\ne: ! plain\n",
        "? a\n: b\n? [x, y]\n: z\n? - p\n  - q\n: r\n? s: 1\n: t\n",
        "a: 'c:\\path'\nb: 'say \"hi\"'\nc: 0x1F\nd: 1.50\ne: \"esc\\n\"\n",
        "a: # after colon\n  b: 1\nc: # before value\n  value\nd: &x # anchor\n  e: 1\n",
    ] {
        assert_eq!(format(input, &Config::default()).unwrap(), input);
    }
}

#[test]
fn comments_keep_their_level_and_blank_lines_collapse() {
    stable(
        "a:\n\n  b: 1\n    # deep\n\n\n  c:\n    d: 2\n      # foot of c\n# root\ne: 3\n\n\n",
        "a:\n  b: 1\n    # deep\n\n  c:\n    d: 2\n      # foot of c\n# root\ne: 3\n",
        &Config::default(),
    );
    stable(
        "- # dash comment\n  a: 1\n- &m\n  b: 2\n",
        "- # dash comment\n  a: 1\n- &m\n  b: 2\n",
        &Config::default(),
    );
    for unchanged in [
        "a:\n  # before value\n  value\nb: !!str # after tag\n  value\n",
        "a:\n  # before block\n  |\n  text\nb: 1\n",
        "podSecurityContext: {}\n  # fsGroup: 2000\nsecurityContext: {}\n",
        "a: one\n  two\n  # under a\nb: 1\n",
    ] {
        stable(unchanged, unchanged, &Config::default());
    }
    stable("a:\n- 1\n# trailing\n", "a:\n  - 1\n# trailing\n", &Config::default());
    stable(
        "#cloud-config\na: 1 #note\n",
        "#cloud-config\na: 1 # note\n",
        &configured("format_comments = true"),
    );
}

#[test]
fn block_collections_starting_with_flow_nodes_stay_block() {
    stable("x:\n- [1, 2]\n- y\n", "x:\n  - [1, 2]\n  - y\n", &Config::default());
    stable("{x: 1}: d\nb: 2\n", "{x: 1}: d\nb: 2\n", &Config::default());
    stable(
        "k:\n  - [a] # c\n  - b\n",
        "k:\n- [a] # c\n- b\n",
        &configured("indent_sequences = false"),
    );
}

#[test]
fn block_scalars_keep_their_exact_content_while_reindenting() {
    stable(
        "s: |\n    keep   spaces  \n\n    trailing\nk: |+\n  x\n\n\nt: |\n  \tx\n    \n  y\nn: |2\n    two\n",
        "s: |\n  keep   spaces  \n\n  trailing\nk: |+\n  x\n\n\nt: |\n  \tx\n    \n  y\nn: |2\n    two\n",
        &Config::default(),
    );
    stable(
        "a:\n    b: >-\n        folded\n          more\n",
        "a:\n  b: >-\n    folded\n      more\n",
        &Config::default(),
    );
    for input in ["a: |+\n  x\n\n\n", "a: |\n  x", "a: |+\n  x"] {
        for config in [Config::default(), configured("final_newline = false")] {
            assert_eq!(format(input, &config).unwrap(), input);
        }
    }
    assert_eq!(format("a: |-\n  x", &Config::default()).unwrap(), "a: |-\n  x\n");
    assert_eq!(
        format("a: |-\n  x\n", &configured("final_newline = false")).unwrap(),
        "a: |-\n  x"
    );
}

#[test]
fn multiline_scalars_reindent_their_continuation_lines() {
    stable(
        "k: one\n      two\n\n      three   # c\nq: \"dq\n   more\"\n",
        "k: one\n  two\n\n  three # c\nq: \"dq\n  more\"\n",
        &Config::default(),
    );
}

#[test]
fn quotes_minimize_escapes_unless_forced() {
    stable(
        "a: 'single'\nb: \"double\"\nc: 'it''s'\nd: 'back\\slash'\n'k': v\n",
        "a: \"single\"\nb: \"double\"\nc: \"it's\"\nd: 'back\\slash'\n\"k\": v\n",
        &Config::default(),
    );
    stable(
        "a: \"it's\"\nb: \"esc\\t\"\n",
        "a: 'it''s'\nb: \"esc\\t\"\n",
        &configured("quote_style = single"),
    );
    stable(
        "a: \"it's\"\nb: \"plain\"\nc: 'it''s'\n",
        "a: \"it's\"\nb: 'plain'\nc: \"it's\"\n",
        &configured("quote_style = auto-prefer-single"),
    );
    stable(
        "a: 'say \"hi\"'\nb: 'back\\slash'\n",
        "a: \"say \\\"hi\\\"\"\nb: \"back\\\\slash\"\n",
        &configured("quote_style = double"),
    );
    stable(
        "a: \"say \\\"hi\\\"\"\n",
        "a: 'say \"hi\"'\n",
        &Config::default(),
    );
}

#[test]
fn long_flow_collections_wrap_into_valid_yaml() {
    let input =
        "- launch: [\"/bin/sh\", \"-c\", \"a long command that cannot fit\"]\n- sleep: 150\n";
    stable(
        input,
        "- launch:\n    [\n      \"/bin/sh\",\n      \"-c\",\n      \"a long command that cannot fit\"\n    ]\n- sleep: 150\n",
        &configured("width = 40"),
    );
    stable(
        "- [aaaaaaaa, bbbbbbbb]\n",
        "-\n  [\n    aaaaaaaa,\n    bbbbbbbb,\n  ]\n",
        &configured("width = 10\ntrailing_comma = true"),
    );
    stable(
        "a: [1, # one\n  2]\n",
        "a: [1, # one\n  2]\n",
        &Config::default(),
    );
    stable(
        "key: [aaaa, bbbb] # c\n",
        "key: # c\n  [\n    aaaa,\n    bbbb\n  ]\n",
        &configured("width = 10"),
    );
    stable(
        "&r [cccc, dddd]\n",
        "&r\n[\n  cccc,\n  dddd\n]\n",
        &configured("width = 10"),
    );
    stable(
        "k: [[aaaaaaa, bbbbb], x]\n",
        "k:\n  [\n    [\n      aaaaaaa,\n      bbbbb\n    ],\n    x\n  ]\n",
        &configured("width = 20"),
    );
}

#[test]
fn flow_spacing_and_line_joining_follow_configuration() {
    stable(
        "a: [one, two]\nb: {c: 1}\n",
        "a: [ one, two ]\nb: { c: 1 }\n",
        &configured("brace_spacing = true\nbracket_spacing = true"),
    );
    stable(
        "a: {x: 1,\n    y: 2}\n",
        "a:\n  {\n    x: 1,\n    y: 2\n  }\n",
        &Config::default(),
    );
    stable(
        "a: {x: 1,\n    y: 2}\n",
        "a: {x: 1, y: 2}\n",
        &configured("prefer_single_line = true"),
    );
}

#[test]
fn sequence_dash_and_comment_settings_apply() {
    stable(
        "a:\n  - x\n",
        "a:\n- x\n",
        &configured("indent_sequences = false"),
    );
    stable(
        "a:\n- b: 1\n  c: 2\n",
        "a:\n    -   b: 1\n        c: 2\n",
        &configured("indent = 4\ndash_spacing = indent"),
    );
    stable(
        "a: 1 #note\n#!keep\n",
        "a: 1 # note\n#!keep\n",
        &configured("format_comments = true"),
    );
}

#[test]
fn final_newline_line_endings_and_bom_are_applied() {
    assert_eq!(
        format("a: 1\n", &configured("final_newline = false")).unwrap(),
        "a: 1"
    );
    assert_eq!(format("a: 1", &Config::default()).unwrap(), "a: 1\n");
    assert_eq!(
        format("a: 1\nb: 2\n", &configured("line_endings = windows")).unwrap(),
        "a: 1\r\nb: 2\r\n"
    );
    assert_eq!(
        format("a: 1\r\nb: |\r\n  x\r\n", &Config::default()).unwrap(),
        "a: 1\nb: |\n  x\n"
    );
    assert_eq!(
        format("\u{feff}a:   1\n", &Config::default()).unwrap(),
        "\u{feff}a: 1\n"
    );
    assert_eq!(format("", &Config::default()).unwrap(), "");
    assert_eq!(format("\n\n", &Config::default()).unwrap(), "");
    assert_eq!(format("# only\n", &Config::default()).unwrap(), "# only\n");
}
