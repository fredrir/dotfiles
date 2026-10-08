#![forbid(unsafe_code)]

#[cfg(unix)]
#[test]
fn diagnostic_paths_preserve_non_unicode_filename_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use dotfmt_core::diagnostic::DiagnosticKind;
    use dotfmt_json::{config::Config, dialect::Dialect, format};

    let path = PathBuf::from(OsString::from_vec(b"source-\xff.json".to_vec()));
    let error = format(&path, b"[1,]", &Config::default(), false, Dialect::Json)
        .err()
        .unwrap();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.path, Some(path));
    assert_eq!((error.line, error.column), (Some(1), Some(4)));
}
