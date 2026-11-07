#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use super::guarded;

#[test]
fn panicking_job_becomes_an_internal_diagnostic_for_its_file() {
    let path = Path::new("bad.lua");
    let result: Result<(), Diagnostic> = guarded(path, || panic!("engine panic"));

    let error = result.unwrap_err();

    assert_eq!(error.kind, DiagnosticKind::Internal);
    assert_eq!(error.message, "formatting worker panicked");
    assert_eq!(error.path.as_deref(), Some(path));
}

#[test]
fn guarded_jobs_keep_their_result_and_gain_the_path() {
    let path = Path::new("dir/a.json");

    assert_eq!(guarded(path, || Ok(7)), Ok(7));

    let error = guarded::<()>(path, || {
        Err(Diagnostic::new(DiagnosticKind::Syntax, "unexpected }"))
    })
    .unwrap_err();

    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.message, "unexpected }");
    assert_eq!(error.path.as_deref(), Some(path));

    let located = Diagnostic::new(DiagnosticKind::Io, "denied").with_path("kept");
    let error = guarded::<()>(path, || Err(located)).unwrap_err();

    assert_eq!(error.path.as_deref(), Some(Path::new("kept")));
}
