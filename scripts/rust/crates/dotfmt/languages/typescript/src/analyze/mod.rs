use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{Analysis, Applicability, Fix};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;

use crate::config::Config;
use crate::dialect::Dialect;
use crate::project;
use crate::source;
use crate::tsconfig;

mod context;
mod imports;
mod locals;
mod organize;

use context::Context;

/// Analysis inputs that need project context.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub fix: bool,
    pub unsafe_fixes: bool,
    /// Run `tsc` inside the language crate.
    pub type_aware: bool,
    /// Include project-wide unused-export findings and fixes.
    pub project_analysis: bool,
    /// TypeScript diagnostics forwarded by the editor, as `(code, start, end)`.
    pub type_diagnostics: Vec<TypeDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDiagnostic {
    pub code: u32,
    pub message: String,
    pub start: usize,
    pub end: usize,
}

/// Returns findings and explicit fixes for a document.
///
/// Formatting never calls this function; only `--fix`, `--diagnose`, and code
/// actions do.
pub fn analyze(
    path: &Path,
    input: &str,
    config: &Config,
    dialect: Dialect,
    options: &Options,
) -> Result<Analysis, Diagnostic> {
    let (bom, text) = match input.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, input),
    };
    let mut adjusted_options;
    let options = if bom {
        adjusted_options = options.clone();
        for diagnostic in &mut adjusted_options.type_diagnostics {
            diagnostic.start = diagnostic.start.saturating_sub('\u{feff}'.len_utf8());
            diagnostic.end = diagnostic.end.saturating_sub('\u{feff}'.len_utf8());
        }
        &adjusted_options
    } else {
        options
    };
    let analysis = analyze_text(path, text, config, dialect, options)?;
    Ok(if bom {
        offset_analysis(analysis, '\u{feff}'.len_utf8())
    } else {
        analysis
    })
}

fn analyze_text(
    path: &Path,
    text: &str,
    config: &Config,
    dialect: Dialect,
    options: &Options,
) -> Result<Analysis, Diagnostic> {
    if crate::source::nesting_exceeded(text) {
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "input nesting exceeds the supported limit",
        ));
    }
    let source_type = dialect
        .resolve(config.dialect, path)
        .source_type_for_path(path);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if let Some(error) = parsed.diagnostics.iter().next() {
        return Err(source::syntax_diagnostic(text, error));
    }

    let semantic_return = SemanticBuilder::new()
        .with_build_nodes(true)
        .with_class_table(true)
        .build(&parsed.program);
    let semantic = semantic_return.semantic;
    let tsconfig = tsconfig::load_for(path);
    let type_diagnostics = resolve_type_diagnostics(path, options);
    let context = Context::new(
        text,
        path,
        config,
        options,
        parsed.program.source_type,
        &parsed.program,
        &semantic,
        &parsed.program.comments,
        tsconfig,
        type_diagnostics,
    );

    let mut analysis = Analysis::default();
    let imports = imports::analyze(&context);
    analysis.findings.extend(imports.findings);
    locals::analyze(&context, &mut analysis);
    for diagnostic in &context.type_diagnostics {
        let handled = matches!(diagnostic.code, 6133 | 6192 | 6196 | 6198 | 6199 | 6205)
            && analysis.findings.iter().any(|finding| {
                finding.start < diagnostic.end.max(diagnostic.start.saturating_add(1))
                    && finding.end.max(finding.start.saturating_add(1)) > diagnostic.start
            });
        if handled {
            continue;
        }
        analysis.findings.push(dotfmt_core::edit::Finding::new(
            format!("tsc-{}", diagnostic.code),
            diagnostic.message.clone(),
            dotfmt_core::edit::Severity::Error,
            diagnostic.start,
            diagnostic.end,
        ));
    }
    if options.project_analysis
        && let Ok(exports) = project::unused_exports_for_analysis(path)
    {
        analysis.extend(exports);
    }

    let organize = organize::plan(&context, &imports.changes);
    if !organize.edits.is_empty() {
        let finding = imports
            .changes
            .iter()
            .filter(|change| organize.covered.contains(&change.decl_start))
            .find_map(|change| change.finding.clone());
        let mut fix = Fix::new(
            "Organize imports",
            "source.organizeImports.dotfmt",
            Applicability::Safe,
            organize.edits,
        );
        fix.finding = finding;
        analysis.fixes.push(fix);
    }
    for change in &imports.changes {
        if organize.covered.contains(&change.decl_start) {
            continue;
        }
        let mut fix = Fix::new(
            &change.title,
            "quickfix",
            Applicability::Safe,
            vec![change.edit.clone()],
        );
        fix.finding = change.finding.clone();
        analysis.fixes.push(fix);
    }

    if !options.fix {
        analysis.fixes.clear();
    } else if !options.unsafe_fixes {
        analysis
            .fixes
            .retain(|fix| fix.applicability != Applicability::Unsafe);
    }

    analysis.findings.sort_by(|left, right| {
        (left.start, left.end, left.code.as_str()).cmp(&(
            right.start,
            right.end,
            right.code.as_str(),
        ))
    });
    analysis.findings.dedup_by(|left, right| {
        left.code == right.code
            && left.start == right.start
            && left.end == right.end
            && left.message == right.message
    });
    analysis.fixes.sort_by_key(|fix| {
        (
            fix.kind.clone(),
            fix.edits.first().map_or(0, |edit| edit.start),
        )
    });
    Ok(analysis)
}

fn resolve_type_diagnostics(path: &Path, options: &Options) -> Vec<TypeDiagnostic> {
    if !options.type_diagnostics.is_empty() || !options.type_aware {
        return options.type_diagnostics.clone();
    }
    let Some(root) = project::discover_root(path) else {
        return Vec::new();
    };
    let Some(tsconfig) = tsconfig::nearest(path) else {
        return Vec::new();
    };
    project::type_diagnostics_for(path, &root, &tsconfig).unwrap_or_default()
}

fn offset_analysis(mut analysis: Analysis, offset: usize) -> Analysis {
    for finding in &mut analysis.findings {
        finding.start += offset;
        finding.end += offset;
        for related in &mut finding.related {
            related.start += offset;
            related.end += offset;
        }
    }
    for fix in &mut analysis.fixes {
        for edit in &mut fix.edits {
            edit.start += offset;
            edit.end += offset;
        }
        if let Some(finding) = &mut fix.finding {
            finding.start += offset;
            finding.end += offset;
            for related in &mut finding.related {
                related.start += offset;
                related.end += offset;
            }
        }
    }
    analysis
}
