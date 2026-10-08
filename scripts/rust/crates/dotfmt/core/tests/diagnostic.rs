#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_core::config::{CONFIG_NAME, Resolver};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

#[test]
fn configuration_errors_retain_source_locations_without_parsing_display_text() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(CONFIG_NAME);
    fs::write(&path, "lua {\n enabled = perhaps\n}\n").unwrap();
    let error = Resolver::with_paths(root.path().to_path_buf(), None)
        .for_directory(root.path())
        .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(error.path.as_deref(), Some(path.as_path()));
    assert_eq!(error.line, Some(2));
    assert_eq!(error.column, None);
    assert_eq!(error.message, "enabled must be true or false");
}

#[test]
fn diagnostics_render_only_available_context() {
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, "unexpected token");
    assert_eq!(diagnostic.to_string(), "unexpected token");
    let diagnostic = diagnostic.with_path(Path::new("source.lua"));
    assert_eq!(diagnostic.to_string(), "source.lua: unexpected token");
    assert_eq!(
        diagnostic.with_location(12, Some(3)).to_string(),
        "source.lua:12:3: unexpected token"
    );
}

#[test]
fn unreadable_configuration_is_a_path_aware_io_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(CONFIG_NAME);
    fs::create_dir(&path).unwrap();
    let error = Resolver::with_paths(root.path().to_path_buf(), None)
        .for_directory(root.path())
        .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Io);
    assert_eq!(error.path.as_deref(), Some(path.as_path()));
    assert_eq!(error.line, None);
}
