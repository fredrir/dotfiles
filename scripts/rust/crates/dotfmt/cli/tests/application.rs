#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use dotfmt::app::{Buffer, Operation, Outcome, Repair, Request, Session};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_core::language::Language;
use testkit::tree_pairs;

fn format(targets: &[&str], buffer: Option<Buffer>) -> Request {
    Request {
        operation: Operation::Format {
            targets: targets.iter().map(PathBuf::from).collect(),
            buffer,
            check: false,
        },
        languages: Vec::new(),
        dialect: None,
    }
}

fn buffer(path: &str, input: &[u8]) -> Buffer {
    Buffer {
        path: path.into(),
        input: input.to_vec(),
        editor: false,
    }
}

#[test]
fn preparation_and_validation_are_read_only_and_execution_uses_the_captured_directory() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("a.json", "{\"a\":1}")]);
    let mut session = Session::with_paths(root.path().into(), None);
    let prepared = session
        .prepare(format(&["a.json"], None))
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\"a\":1}"
    );
    let plan = session
        .validate(prepared)
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\"a\":1}"
    );
    let outcome = session.execute(plan).unwrap();
    assert!(!outcome.failed());
    let Outcome::Formatted { files, .. } = outcome else {
        panic!("expected formatting")
    };
    assert_eq!(files[0].path, PathBuf::from("a.json"));
    assert!(files[0].result.as_ref().unwrap().changed);
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\n  \"a\": 1\n}\n"
    );
}

#[test]
fn invalid_enabled_settings_block_a_mixed_request_before_execution() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\nlua { indent = bad }\n"),
        ("a.json", "{\"a\":1}"),
    ]);
    let mut session = Session::with_paths(root.path().into(), None);
    let prepared = session
        .prepare(format(&["a.json"], Some(buffer("input.json", b"{}"))))
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    let errors = match session.validate(prepared) {
        Ok(_) => panic!("invalid settings were accepted"),
        Err(errors) => errors,
    };
    assert_eq!(errors[0].kind, DiagnosticKind::Configuration);
    assert_eq!(
        errors[0].path.as_deref(),
        Some(root.path().join("dotfmt.dotfile").as_path())
    );
    assert_eq!(errors[0].line, Some(2));
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\"a\":1}"
    );
}

#[test]
fn syntax_failures_are_structured_and_valid_files_continue() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\nlua {}\n"),
        ("a.lua", "local ="),
        ("b.json", "{\"b\":2}"),
    ]);
    let mut session = Session::with_paths(root.path().into(), None);
    let outcome = session
        .run(format(
            &["a.lua", "b.json"],
            Some(buffer("input.json", b"{")),
        ))
        .unwrap();
    assert!(outcome.failed());
    let Outcome::Formatted {
        files,
        buffer: Some(buffer),
        ..
    } = outcome
    else {
        panic!("expected formatting")
    };
    assert_eq!(buffer.result.unwrap_err().kind, DiagnosticKind::Syntax);
    assert_eq!(
        files[0].result.as_ref().unwrap_err().kind,
        DiagnosticKind::Syntax
    );
    assert!(files[1].result.as_ref().unwrap().changed);
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local ="
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.json")).unwrap(),
        "{\n  \"b\": 2\n}\n"
    );
}

#[test]
fn repeated_session_requests_keep_dialect_overrides_separate() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json { dialect = jsonc }\n")]);
    let mut session = Session::with_paths(root.path().into(), None);
    for (dialect, failed) in [
        (Some("json"), true),
        (None, false),
        (Some("jsonc"), false),
        (Some("json"), true),
    ] {
        let mut request = format(&[], Some(buffer("input.json", b"// note\n{}")));
        request.dialect = dialect.map(str::to_owned);
        assert_eq!(session.run(request).unwrap().failed(), failed);
    }
    let owned = session
        .run(Request {
            operation: Operation::Owns {
                paths: vec!["new.json".into(), "other.lua".into()],
            },
            languages: vec![Language::Json],
            dialect: None,
        })
        .unwrap();
    let Outcome::Owned(paths) = owned else {
        panic!("expected ownership")
    };
    assert_eq!(paths, vec![PathBuf::from("new.json")]);
}

#[test]
fn prepared_requests_can_move_between_sessions_without_losing_their_configuration() {
    let one = tree_pairs(&[
        ("dotfmt.dotfile", "json { indent = 1 }\n"),
        ("a.json", "{\"a\":1}"),
    ]);
    let other = tree_pairs(&[("dotfmt.dotfile", "json { indent = 4 }\n")]);
    let prepared = Session::with_paths(one.path().into(), None)
        .prepare(format(&["a.json"], None))
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    let mut session = Session::with_paths(other.path().into(), None);
    let plan = session
        .validate(prepared)
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    assert!(!session.execute(plan).unwrap().failed());
    assert_eq!(
        fs::read_to_string(one.path().join("a.json")).unwrap(),
        "{\n \"a\": 1\n}\n"
    );
    let outcome = session
        .run(format(&[], Some(buffer("new.json", b"{\"a\":1}"))))
        .unwrap();
    let Outcome::Formatted {
        buffer: Some(buffer),
        ..
    } = outcome
    else {
        panic!("expected buffer")
    };
    assert_eq!(buffer.result.unwrap().output, b"{\n    \"a\": 1\n}\n");
}

#[test]
fn excluded_buffers_preserve_arbitrary_bytes_and_editor_repairs_stay_structured() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\nexcluded_files { skip.json }\n")]);
    let mut session = Session::with_paths(root.path().into(), None);
    let outcome = session
        .run(format(&[], Some(buffer("skip.json", &[255, 0]))))
        .unwrap();
    let Outcome::Formatted {
        buffer: Some(buffer),
        ..
    } = outcome
    else {
        panic!("expected buffer")
    };
    assert_eq!(buffer.result.unwrap().output, [255, 0]);
    let mut input = Buffer {
        path: "input.json".into(),
        input: b"{key:'value',}".to_vec(),
        editor: false,
    };
    input.editor = true;
    let outcome = session.run(format(&[], Some(input))).unwrap();
    let Outcome::Formatted {
        buffer: Some(buffer),
        ..
    } = outcome
    else {
        panic!("expected buffer")
    };
    let change = buffer.result.unwrap().change;
    assert!(change.repairs.of(Repair::Key) > 0);
    assert!(change.repairs.of(Repair::Quote) > 0);
}

#[test]
fn discovery_errors_keep_io_kind_and_the_supplied_path() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n")]);
    let mut session = Session::with_paths(root.path().into(), None);
    let errors = session.run(format(&["missing.json"], None)).unwrap_err();
    assert_eq!(errors[0].kind, DiagnosticKind::Io);
    assert_eq!(
        errors[0].path.as_deref(),
        Some(std::path::Path::new("missing.json"))
    );
}

#[test]
fn cached_markdown_context_stays_local_to_its_directory_and_respects_overrides() {
    use dotfmt_markdown::{config::Config, dialect::Dialect};
    let root = tree_pairs(&[("dotfmt.dotfile", "markdown {}\n")]);
    fs::create_dir_all(root.path().join("vault/.obsidian")).unwrap();
    let text = "> [!custom]- Custom title\n> Body with [[Note]].\n>\n> ## Heading\n> body";
    let gfm = dotfmt_markdown::format(
        text,
        &Config {
            dialect: Dialect::Gfm,
            ..Config::default()
        },
    )
    .unwrap();
    let obsidian = dotfmt_markdown::format(
        text,
        &Config {
            dialect: Dialect::Obsidian,
            ..Config::default()
        },
    )
    .unwrap();
    assert_ne!(gfm, obsidian);
    let mut session = Session::with_paths(root.path().into(), None);
    for (path, dialect, expected) in [
        ("ordinary/note.md", None, &gfm),
        ("vault/deep/note.md", None, &obsidian),
        ("another/note.md", None, &gfm),
        ("vault/note.md", Some("gfm"), &gfm),
        ("vault/again.md", None, &obsidian),
    ] {
        let mut request = format(&[], Some(buffer(path, text.as_bytes())));
        request.dialect = dialect.map(str::to_owned);
        let outcome = session.run(request).unwrap();
        let Outcome::Formatted {
            buffer: Some(buffer),
            ..
        } = outcome
        else {
            panic!("expected buffer")
        };
        assert_eq!(buffer.result.unwrap().output, expected.as_bytes(), "{path}");
    }
}
