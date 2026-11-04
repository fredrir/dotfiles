#![forbid(unsafe_code)]

use dotfmt_ini::config::{Config, IndentType};
use dotfmt_ini::dialect::Dialect;
use dotfmt_ini::format_with_dialect;

fn formatted(input: &str, config: &Config, dialect: Dialect) -> String {
    let output = format_with_dialect(input, config, dialect).unwrap();
    assert_eq!(
        format_with_dialect(&output, config, dialect).unwrap(),
        output,
        "formatting must settle after one pass"
    );
    output
}

fn default(input: &str, dialect: Dialect) -> String {
    formatted(input, &Config::default(), dialect)
}

#[test]
fn git_config_sections_alignment_quotes_duplicates_and_includes() {
    let input = "[user]\nname=Jane Doe\nemail   = jane@example.com\n[alias]\nco=checkout\nlg = log --graph \\\n       --oneline --decorate\n[include]\npath = ~/.gitconfig.local\n[remote \"origin\"]\nurl = \"https://example.com/repo.git?x=1&y=2\"\nfetch=+refs/heads/*:refs/remotes/origin/*\ndup=one\ndup=two\n[core]\neditor = \"code --wait\"\npath = C:\\Users\\me ; kept\n";
    assert_eq!(
        default(input, Dialect::Git),
        "[user]\n  name  = Jane Doe\n  email = jane@example.com\n[alias]\n  co = checkout\n  lg = log --graph \\\n       --oneline --decorate\n[include]\n  path = ~/.gitconfig.local\n[remote \"origin\"]\n  url   = \"https://example.com/repo.git?x=1&y=2\"\n  fetch = +refs/heads/*:refs/remotes/origin/*\n  dup   = one\n  dup   = two\n[core]\n  editor = \"code --wait\"\n  path   = C:\\Users\\me ; kept\n"
    );
}

#[test]
fn git_continuation_lines_keep_their_leading_bytes() {
    let input = "[section]\nkey = one \\\n\t two \\\n  three\nnext=four\n";
    let output = default(input, Dialect::Git);
    assert_eq!(
        output,
        "[section]\n  key = one \\\n\t two \\\n  three\n  next = four\n"
    );
    assert!(output.contains("\\\n\t two \\\n  three"));
}

#[test]
fn editorconfig_glob_headers_and_special_value_bytes() {
    let input = "root=true\n[*]\nindent_style = space\nindent_size=2\n[*.{js,ts}]\ncharset              = utf-8\ninsert_final_newline=true\n[Makefile]\nindent_style=tab\ntrim_trailing_whitespace = false\n[*.md]\nfoo = value # inline kept\nbar = a;b\nbaz= \"quoted # value\"\n#comment\n";
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::EditorConfig),
        "root = true\n[*]\nindent_style = space\nindent_size  = 2\n[*.{js,ts}]\ncharset              = utf-8\ninsert_final_newline = true\n[Makefile]\nindent_style             = tab\ntrim_trailing_whitespace = false\n[*.md]\nfoo = value # inline kept\nbar = a;b\nbaz = \"quoted # value\"\n# comment\n"
    );
}

#[test]
fn systemd_entries_use_compact_equals_and_keep_inline_hashes() {
    let input = "[Unit]\nDescription=My Service   # inline\nAfter=network.target\n[Service]\nExecStart = /usr/bin/foo --bar=baz\nEnvironment=FOO=bar BAZ=qux\nExecReload=/bin/kill -HUP $MAINPID\nEnvironment=\"A=1\" \"B=2\"\n#comment\nExecStartPre=/usr/bin/prepare \\\n    --flag\n[Install]\nWantedBy=multi-user.target\n";
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::Systemd),
        "[Unit]\nDescription=My Service   # inline\nAfter=network.target\n[Service]\nExecStart=/usr/bin/foo --bar=baz\nEnvironment=FOO=bar BAZ=qux\nExecReload=/bin/kill -HUP $MAINPID\nEnvironment=\"A=1\" \"B=2\"\n# comment\nExecStartPre=/usr/bin/prepare \\\n    --flag\n[Install]\nWantedBy=multi-user.target\n"
    );
}

#[test]
fn ssh_blocks_normalize_equals_forms_and_align_keywords() {
    let input = "Host example.com\nHostName example.com\nUser=alice\nPort = 22\nIdentityFile \"~/.ssh/id key\"\nProxyCommand=\nHost *.internal\nUser bob\nForwardAgent yes\nMatch host db\nUser=carol\nInclude config.d/*\n#comment\n";
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::Ssh),
        "Host example.com\n    HostName     example.com\n    User         alice\n    Port         22\n    IdentityFile \"~/.ssh/id key\"\n    ProxyCommand=\nHost *.internal\n    User         bob\n    ForwardAgent yes\nMatch host db\n    User    carol\n    Include config.d/*\n# comment\n"
    );
}

#[test]
fn ssh_comments_stay_at_column_zero_between_blocks() {
    let input = "Host one\n    User a\n# next\nHost two\n    User b\n";
    assert_eq!(
        default(input, Dialect::Ssh),
        "Host one\n    User a\n# next\nHost two\n    User b\n"
    );
}

#[test]
fn ssh_empty_equals_form_stays_distinct_from_a_bare_keyword() {
    let input = "Host x\nProxyCommand=\nProxyCommand =\nBareKeyword\n";
    assert_eq!(
        default(input, Dialect::Ssh),
        "Host x\n    ProxyCommand=\n    ProxyCommand=\nBareKeyword\n"
    );
}

#[test]
fn generic_ini_preserves_variant_values_and_aligns_keys() {
    let input = "[Section]\nKey=@Variant(\\0\\0\\0\\x7f)\nother  =  @Variant(\\0\\0\\0\\0)\nEmpty=\nSpaced   =   a   b\n#comment\n;another\n";
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::Generic),
        "[Section]\nKey    = @Variant(\\0\\0\\0\\x7f)\nother  = @Variant(\\0\\0\\0\\0)\nEmpty  =\nSpaced = a   b\n# comment\n; another\n"
    );
}

#[test]
fn explicit_indent_and_tabs_apply_to_git_and_ssh() {
    let input = "[section]\nkey=value\n";
    let spaces = Config {
        indent: Some(1),
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &spaces, Dialect::Git),
        "[section]\n key = value\n"
    );
    assert_eq!(
        formatted(input, &spaces, Dialect::Generic),
        "[section]\nkey = value\n"
    );
    let tabs = Config {
        indent_type: Some(IndentType::Tabs),
        ..Config::default()
    };
    assert_eq!(
        formatted("Host x\nHostName x\n", &tabs, Dialect::Ssh),
        "Host x\n\tHostName x\n"
    );
    assert_eq!(
        formatted("[s]\nk=v\n", &spaces, Dialect::EditorConfig),
        "[s]\nk = v\n"
    );
}

#[test]
fn generic_indented_lines_stay_continuations() {
    let input = "[testenv]\nsetenv =\n    FOO=bar\n    BAZ=qux\ncommands =\n    pytest\n";
    assert_eq!(default(input, Dialect::Generic), input);
    assert_eq!(
        default("  stray=1\n", Dialect::Generic),
        "  stray=1\n",
        "an indented line is data, not a top-level entry"
    );
    assert_eq!(
        default(
            "[testenv]\ncommands =\n    [ -f foo ]\n    # keep this value line\n",
            Dialect::Generic
        ),
        "[testenv]\ncommands =\n    [ -f foo ]\n    # keep this value line\n",
        "indented section-like and comment-like lines are continuations"
    );
}

#[test]
fn trailing_space_after_a_backslash_stays_data() {
    assert_eq!(
        default("[s]\nkey = value\\ \nnext = two\n", Dialect::Generic),
        "[s]\nkey  = value\\ \nnext = two\n"
    );
    assert_eq!(
        default("[s]\nkey=value\\ \nnext=two\n", Dialect::Systemd),
        "[s]\nkey=value\\ \nnext=two\n"
    );
    assert_eq!(
        default("[s]\nkey = value\\ \nnext = two\n", Dialect::Git),
        "[s]\n  key  = value\\ \n  next = two\n"
    );
}

#[test]
fn blank_line_runs_are_collapsed_but_never_inserted() {
    let input = "\n\n[one]\na=1\n\n\n\nb=2\n\n[two]\nc=3\n\n";
    let config = Config {
        blank_lines: 1,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::Generic),
        "[one]\na = 1\n\nb = 2\n\n[two]\nc = 3\n"
    );
    let none = Config {
        blank_lines: 0,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &none, Dialect::Generic),
        "[one]\na = 1\nb = 2\n[two]\nc = 3\n"
    );
}

#[test]
fn dropped_blank_lines_merge_alignment_groups_idempotently() {
    let input = "[s]\na=1\n\nlonger=2\n";
    let none = Config {
        blank_lines: 0,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &none, Dialect::Generic),
        "[s]\na      = 1\nlonger = 2\n"
    );
}

#[test]
fn comment_spacing_preserves_hashes_bangs_and_semicolons() {
    let input = "#plain\n##double\n#!shebang\n;;;triple\n;  spaced  \n#\t\n";
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &config, Dialect::Generic),
        "# plain\n##double\n#!shebang\n;;;triple\n;  spaced\n#\n"
    );
}

#[test]
fn inline_comment_like_bytes_are_never_stripped_from_values() {
    let input = "[s]\na = one # two\nb = one ; two\nc = \"one # two\"\n";
    let output = default(input, Dialect::Git);
    assert!(output.contains("a = one # two"));
    assert!(output.contains("b = one ; two"));
    assert!(output.contains("c = \"one # two\""));
}
