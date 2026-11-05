//! Calls into the dotfmt library for one buffer at a time.

use std::path::{Path, PathBuf};

use dotfmt::app::{
    Buffer, FixMode, InspectBufferOutcome, Inspection, Operation, Outcome, Request, Session,
};
use dotfmt_core::diagnostic::Diagnostic;
use dotfmt_core::edit::{Applicability, apply_fixes};
use dotfmt_core::language::Language;
use lsp_types::Uri;

use crate::uri;

/// The dotfmt language for an editor language id.
pub fn language_for(language_id: &str) -> Option<Language> {
    match language_id {
        "bibtex" => Some(Language::Bib),
        "chatagent" | "instructions" | "markdown" | "prompt" | "skill" => Some(Language::Markdown),
        "dockercompose" | "yaml" => Some(Language::Yaml),
        "dotenv" => Some(Language::Env),
        "dotfile" | "properties" => Some(Language::Conf),
        "ignore" => Some(Language::Ignore),
        "ini" => Some(Language::Ini),
        "json" | "jsonc" | "jsonl" => Some(Language::Json),
        "latex" | "tex" => Some(Language::Latex),
        "lua" => Some(Language::Lua),
        "toml" => Some(Language::Toml),
        "typescript" | "typescriptreact" => Some(Language::TypeScript),
        "javascript" | "javascriptreact" => Some(Language::Javascript),
        "css" | "scss" | "sass" | "less" => Some(Language::Css),
        "html" | "vue" | "svelte" | "astro" => Some(Language::Html),
        _ => None,
    }
}

/// Forced languages for buffers whose filename cannot carry the language.
pub fn forced(path: &Path, uri: &Uri, language_id: &str) -> Vec<Language> {
    if !crate::uri::is_file(uri) || Language::for_path(path).is_none() {
        language_for(language_id).into_iter().collect()
    } else {
        Vec::new()
    }
}

/// The path a buffer is formatted as; untitled buffers get a stable stdin name.
pub fn buffer_path(uri: &Uri, language_id: &str) -> Option<PathBuf> {
    uri::path(uri).or_else(|| {
        let path = match language_id {
            "typescriptreact" => Path::new("stdin.tsx"),
            "javascriptreact" => Path::new("stdin.jsx"),
            "scss" => Path::new("stdin.scss"),
            "sass" => Path::new("stdin.sass"),
            "less" => Path::new("stdin.less"),
            "vue" => Path::new("stdin.vue"),
            "svelte" => Path::new("stdin.svelte"),
            "astro" => Path::new("stdin.astro"),
            _ => language_for(language_id)?.default_stdin(),
        };
        Some(path.to_path_buf())
    })
}

#[allow(clippy::collapsible_match)]
pub fn format(
    session: &mut Session,
    path: &Path,
    text: &str,
    languages: Vec<Language>,
) -> Result<String, Vec<Diagnostic>> {
    let request = Request {
        operation: Operation::Format {
            targets: Vec::new(),
            buffer: Some(Buffer {
                path: path.to_path_buf(),
                input: text.as_bytes().to_vec(),
            }),
            check: false,
        },
        languages,
        dialect: None,
        clean: false,
    };
    match session.run(request)? {
        Outcome::Formatted { buffer, .. } => match buffer {
            Some(outcome) => formatted(outcome),
            None => Ok(text.to_owned()),
        },
        _ => Ok(text.to_owned()),
    }
}

/// The LSP publishes project `tsc` diagnostics from its own worker; the
/// language-crate findings with a `tsc-` code would duplicate them.
fn drop_tsc_findings(mut inspection: Inspection) -> Inspection {
    inspection
        .findings
        .retain(|finding| !finding.code.starts_with("tsc-"));
    inspection
}

fn formatted(outcome: dotfmt::app::BufferOutcome) -> Result<String, Vec<Diagnostic>> {
    outcome
        .result
        .map(|change| String::from_utf8_lossy(&change.output).into_owned())
        .map_err(|error| vec![error])
}

/// Applies the safe `source.organizeImports.dotfmt` fixes, then formats.
pub fn organize(
    session: &mut Session,
    path: &Path,
    text: &str,
    languages: Vec<Language>,
) -> Result<String, Vec<Diagnostic>> {
    let inspection = inspect(session, path, text, languages.clone(), FixMode::Unsafe)?;
    let selected: Vec<&dotfmt_core::edit::Fix> = inspection
        .fixes
        .iter()
        .filter(|fix| {
            fix.kind.contains("organizeImports") && fix.applicability == Applicability::Safe
        })
        .collect();
    if selected.is_empty() {
        return format(session, path, &inspection.before, languages);
    }
    let organized = apply_fixes(text, &selected).map_err(|error| {
        vec![Diagnostic::new(
            dotfmt_core::diagnostic::DiagnosticKind::Internal,
            format!("fix edits: {error}"),
        )]
    })?;
    format(session, path, &organized, languages)
}

#[allow(clippy::collapsible_match)]
pub fn inspect(
    session: &mut Session,
    path: &Path,
    text: &str,
    languages: Vec<Language>,
    fix: FixMode,
) -> Result<Inspection, Vec<Diagnostic>> {
    let request = Request {
        operation: Operation::Inspect {
            targets: Vec::new(),
            buffer: Some(Buffer {
                path: path.to_path_buf(),
                input: text.as_bytes().to_vec(),
            }),
            fix,
            diff: false,
            json: false,
            // The LSP publishes project tsc diagnostics from its own worker.
            type_aware: Some(false),
        },
        languages,
        dialect: None,
        clean: false,
    };
    match session.run(request)? {
        Outcome::Inspected { buffer, .. } => match buffer {
            Some(InspectBufferOutcome { result, .. }) => {
                result.map(drop_tsc_findings).map_err(|error| vec![error])
            }
            None => Ok(Inspection {
                findings: Vec::new(),
                fixes: Vec::new(),
                applied: Vec::new(),
                before: text.to_owned(),
                after: text.to_owned(),
                changed: false,
                diff: None,
            }),
        },
        _ => Ok(Inspection {
            findings: Vec::new(),
            fixes: Vec::new(),
            applied: Vec::new(),
            before: text.to_owned(),
            after: text.to_owned(),
            changed: false,
            diff: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn maps_editor_language_ids() {
        assert_eq!(language_for("typescriptreact"), Some(Language::TypeScript));
        assert_eq!(language_for("svelte"), Some(Language::Html));
        assert_eq!(language_for("scss"), Some(Language::Css));
        assert_eq!(language_for("plaintext"), None);
    }

    #[test]
    fn untitled_buffers_get_a_stdin_name() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(
            buffer_path(&uri, "json").as_deref(),
            Some(Path::new("stdin.json"))
        );
        assert_eq!(
            buffer_path(&uri, "typescriptreact").as_deref(),
            Some(Path::new("stdin.tsx"))
        );
        assert_eq!(
            buffer_path(&uri, "scss").as_deref(),
            Some(Path::new("stdin.scss"))
        );
        assert!(forced(Path::new("stdin.json"), &uri, "json").contains(&Language::Json));
    }
}
