mod common;
mod maps;
mod roles_fonts;

use std::fmt::{self, Display, Formatter};

use dotfile_schema::{
    ClassifiedPath, Domain, DomainClassifier, DomainLocation, LoweredFile, LoweringError,
    PathClassification, classify_static, lower_path,
};
use dotfile_source::{Diagnostic, RepoPath, Severity, SourceText, sort_diagnostics};
use dotfile_syntax::{Parse, parse};

use crate::model::{ThemeDocument, ThemeFileKind, ThemeIdentity, ThemeMap};
use crate::{ThemeHir, lower_theme_hir};

use self::common::Context;

/// A validated path classification. Profile identity is present exactly for
/// `ThemeFileKind::Profile` and comes solely from the immediate filename.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassifiedThemePath {
    pub kind: ThemeFileKind,
    pub profile_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemePathError {
    Unregistered,
    InvalidProfileName,
}

impl Display for ThemePathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unregistered => "path is not a registered theme-definition source",
            Self::InvalidProfileName => "theme profile basename is not an IDENT",
        })
    }
}

impl std::error::Error for ThemePathError {}

/// Failure to use an already parsed tree as the authoritative syntax for the
/// exact repository path and source bytes being lowered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeLoweringError {
    MismatchedParse,
    MismatchedSchema,
}

impl Display for ThemeLoweringError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MismatchedParse => {
                formatter.write_str("the syntax tree was parsed from a different path or source")
            }
            Self::MismatchedSchema => formatter.write_str(
                "the schema HIR was lowered from different source bytes, path, or classification",
            ),
        }
    }
}

impl std::error::Error for ThemeLoweringError {}

/// Projects the authoritative schema classifier onto the eight theme source
/// domains. This function never performs a second path-pattern match.
pub fn classify_theme_path(path: &RepoPath) -> Result<ClassifiedThemePath, ThemePathError> {
    let PathClassification::Known(classification) = classify_static(path) else {
        return Err(ThemePathError::Unregistered);
    };
    project_theme_classification(&classification).ok_or(ThemePathError::Unregistered)
}

fn project_theme_classification(classification: &ClassifiedPath) -> Option<ClassifiedThemePath> {
    let kind = match classification.domain {
        Domain::ThemeRoles => ThemeFileKind::Roles,
        Domain::ThemeFonts => ThemeFileKind::Fonts,
        Domain::ThemeProfiles => ThemeFileKind::Profile,
        Domain::ThemeMapCatppuccin => ThemeFileKind::CatppuccinMap,
        Domain::ThemeMapEza => ThemeFileKind::EzaMap,
        Domain::ThemeMapGtk => ThemeFileKind::GtkMap,
        Domain::ThemeMapKde => ThemeFileKind::KdeMap,
        Domain::ThemeMapObsidian => ThemeFileKind::ObsidianMap,
        _ => return None,
    };
    let profile_name = match (&classification.domain, &classification.location) {
        (Domain::ThemeProfiles, DomainLocation::ThemeProfile { theme }) => Some(theme.clone()),
        (Domain::ThemeProfiles, _) => return None,
        (_, _) => None,
    };
    Some(ClassifiedThemePath { kind, profile_name })
}

/// One unified M2 theme lowering.
///
/// `schema` is the authoritative, tolerant, source-mapped HIR produced by
/// `dotfile-schema`. `partial_document` retains a specialized projection when
/// its required typed skeleton could be built even if an independent source
/// error exists. `document` is sealed to error-free input for compatibility
/// with validated consumers.
#[derive(Clone, Debug)]
pub struct ThemeLowering {
    path: RepoPath,
    source: SourceText,
    kind: Option<ThemeFileKind>,
    schema: LoweredFile,
    hir: Option<ThemeHir>,
    partial_document: Option<ThemeDocument>,
    document: Option<ThemeDocument>,
    diagnostics: Vec<Diagnostic>,
}

impl ThemeLowering {
    pub fn kind(&self) -> Option<ThemeFileKind> {
        self.kind
    }

    pub fn path(&self) -> &RepoPath {
        &self.path
    }

    pub fn schema(&self) -> &LoweredFile {
        &self.schema
    }

    pub fn hir(&self) -> Option<&ThemeHir> {
        self.hir.as_ref()
    }

    pub fn partial_document(&self) -> Option<&ThemeDocument> {
        self.partial_document.as_ref()
    }

    pub fn document(&self) -> Option<&ThemeDocument> {
        self.document.as_ref()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn source_map(&self) -> &dotfile_schema::SourceMap {
        self.schema.source_map()
    }

    /// Seals the specialized projection only when syntax, schema, and
    /// immediate theme validation all succeeded.
    pub fn into_validated(self) -> Result<ValidatedThemeFile, ThemeValidationFailure> {
        let kind_is_authoritative = self.kind.is_some_and(|kind| {
            self.schema
                .classification()
                .and_then(project_theme_classification)
                .is_some_and(|classification| classification.kind == kind)
        });
        let hir_is_authoritative = self
            .kind
            .zip(self.hir.as_ref())
            .is_some_and(|(kind, hir)| hir.is_authoritative_for(kind, &self.schema));
        let document_is_consistent = self
            .kind
            .zip(self.document.as_ref())
            .is_some_and(|(kind, document)| document_kind(document) == kind);
        if self.has_errors()
            || self.schema.has_errors()
            || !self.schema.was_lowered_from(&self.source)
            || self.schema.path() != &self.path
            || !schema_snapshot_is_authoritative(&self.schema)
            || !kind_is_authoritative
            || !hir_is_authoritative
            || !document_is_consistent
            || self.document.as_ref() != self.partial_document.as_ref()
            || self.document.is_none()
            || self.hir.as_ref().is_none_or(ThemeHir::has_poison)
        {
            Err(ThemeValidationFailure {
                lowering: Box::new(self),
            })
        } else {
            Ok(ValidatedThemeFile { lowering: self })
        }
    }

    /// Deterministic, compact projection used by conformance fixtures. This
    /// is intentionally not a persistence format: it exposes the immediate
    /// typed shape, source order, and raw byte spans without pretending to be
    /// resolved theme IR.
    pub fn dump_json(&self) -> serde_json::Value {
        crate::model::dump_lowering(self)
    }
}

/// A source proven valid through the complete immediate theme boundary.
/// Construction is restricted to [`ThemeLowering::into_validated`].
#[derive(Clone, Debug)]
pub struct ValidatedThemeFile {
    lowering: ThemeLowering,
}

impl ValidatedThemeFile {
    pub fn document(&self) -> &ThemeDocument {
        self.lowering
            .document
            .as_ref()
            .expect("validated theme has a document")
    }

    pub fn hir(&self) -> &ThemeHir {
        self.lowering
            .hir
            .as_ref()
            .expect("validated theme has tolerant HIR")
    }

    pub fn schema(&self) -> &LoweredFile {
        &self.lowering.schema
    }

    pub fn into_lowering(self) -> ThemeLowering {
        self.lowering
    }
}

#[derive(Clone, Debug)]
pub struct ThemeValidationFailure {
    lowering: Box<ThemeLowering>,
}

impl ThemeValidationFailure {
    pub fn lowering(&self) -> &ThemeLowering {
        &self.lowering
    }

    pub fn into_lowering(self) -> ThemeLowering {
        *self.lowering
    }
}

/// Parses and lowers one path-selected theme source.
pub fn lower_theme_file(path: &RepoPath, source: &SourceText) -> ThemeLowering {
    let parsed = parse(path, source);
    lower_parsed_theme_file(path, source, &parsed)
        .expect("a freshly parsed theme source always matches its syntax tree")
}

/// Lowers an already parsed source, preserving the parser diagnostics in the
/// returned diagnostic stream.
pub fn lower_parsed_theme_file(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
) -> Result<ThemeLowering, ThemeLoweringError> {
    if !parsed.was_parsed_from(path, source) {
        return Err(ThemeLoweringError::MismatchedParse);
    }
    let schema = lower_path(path, source, parsed, &DomainClassifier::without_groups())
        .map_err(ThemeLoweringError::from)?;
    lower_schema_theme_file(source, parsed, schema)
}

fn schema_snapshot_is_authoritative(schema: &LoweredFile) -> bool {
    let expected = match classify_static(schema.path()) {
        PathClassification::Known(classification) => Some(classification),
        PathClassification::UnknownDotfile | PathClassification::NotDotfile => None,
    };
    if schema.classification() != expected.as_ref() {
        return false;
    }
    let deferred_is_authoritative = expected
        .as_ref()
        .and_then(project_theme_classification)
        .is_none_or(|_| schema.deferred_snapshot_is_authoritative());
    deferred_is_authoritative
        && schema.diagnostics().iter().all(|diagnostic| {
            diagnostic.primary_span.path == schema.path().as_str()
                && diagnostic
                    .related_spans
                    .iter()
                    .all(|span| span.path == schema.path().as_str())
                && diagnostic.fix.as_ref().is_none_or(|fix| {
                    fix.edits
                        .iter()
                        .all(|edit| edit.path == schema.path().as_str())
                })
        })
}

/// Adds the specialized theme projection to an authoritative schema
/// lowering. This is the integration entry point for a repository pipeline
/// that has already classified and lowered the file once.
pub fn lower_schema_theme_file(
    source: &SourceText,
    parsed: &Parse,
    mut schema: LoweredFile,
) -> Result<ThemeLowering, ThemeLoweringError> {
    if !parsed.was_parsed_from(schema.path(), source) {
        return Err(ThemeLoweringError::MismatchedParse);
    }
    if !schema.was_lowered_from(source) || !schema_snapshot_is_authoritative(&schema) {
        return Err(ThemeLoweringError::MismatchedSchema);
    }
    let path = schema.path().clone();
    let classification = schema
        .classification()
        .and_then(project_theme_classification);
    let kind = classification.as_ref().map(|item| item.kind);
    let mut diagnostics = parsed.diagnostics().to_vec();
    diagnostics.extend(schema.diagnostics().iter().cloned());
    if classification.is_none() {
        sort_diagnostics(&mut diagnostics);
        return Ok(ThemeLowering {
            path,
            source: source.clone(),
            kind,
            schema,
            hir: None,
            partial_document: None,
            document: None,
            diagnostics,
        });
    }

    let classification = classification.expect("checked classification");
    let semantic_identity = classification
        .profile_name
        .as_ref()
        .map(|name| format!("theme:{name}"));
    let ast = parsed.ast(source);
    let mut context = Context::new(&path, source, parsed.line_index(), parsed.cst());
    let partial_document = match classification.kind {
        ThemeFileKind::Roles => {
            roles_fonts::lower_roles(&mut context, ast).map(ThemeDocument::Roles)
        }
        ThemeFileKind::Fonts => {
            roles_fonts::lower_fonts(&mut context, ast).map(ThemeDocument::Fonts)
        }
        ThemeFileKind::Profile => {
            let identity = ThemeIdentity {
                name: classification
                    .profile_name
                    .clone()
                    .expect("profile classification has name"),
                path: path.clone(),
            };
            roles_fonts::lower_profile(&mut context, ast, identity)
                .map(Box::new)
                .map(ThemeDocument::Profile)
        }
        ThemeFileKind::CatppuccinMap => maps::lower_catppuccin(&mut context, ast)
            .map(ThemeMap::Catppuccin)
            .map(ThemeDocument::Map),
        ThemeFileKind::EzaMap => maps::lower_eza(&mut context, ast)
            .map(ThemeMap::Eza)
            .map(ThemeDocument::Map),
        ThemeFileKind::GtkMap => maps::lower_gtk(&mut context, ast)
            .map(ThemeMap::Gtk)
            .map(ThemeDocument::Map),
        ThemeFileKind::KdeMap => maps::lower_kde(&mut context, ast)
            .map(ThemeMap::Kde)
            .map(ThemeDocument::Map),
        ThemeFileKind::ObsidianMap => maps::lower_obsidian(&mut context, ast)
            .map(ThemeMap::Obsidian)
            .map(ThemeDocument::Map),
    };

    diagnostics.extend(context.finish());
    if let Some(identity) = semantic_identity {
        for diagnostic in &mut diagnostics {
            if diagnostic.semantic_identity.is_empty() {
                diagnostic.semantic_identity.clone_from(&identity);
            }
        }
    }
    sort_diagnostics(&mut diagnostics);
    let hir = lower_theme_hir(
        classification.kind,
        ast,
        parsed.cst(),
        &mut schema,
        &diagnostics,
    );
    let has_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    Ok(ThemeLowering {
        path,
        source: source.clone(),
        kind: Some(classification.kind),
        schema,
        hir: Some(hir),
        document: if has_errors {
            None
        } else {
            partial_document.clone()
        },
        partial_document,
        diagnostics,
    })
}

fn document_kind(document: &ThemeDocument) -> ThemeFileKind {
    match document {
        ThemeDocument::Roles(_) => ThemeFileKind::Roles,
        ThemeDocument::Fonts(_) => ThemeFileKind::Fonts,
        ThemeDocument::Profile(_) => ThemeFileKind::Profile,
        ThemeDocument::Map(ThemeMap::Catppuccin(_)) => ThemeFileKind::CatppuccinMap,
        ThemeDocument::Map(ThemeMap::Eza(_)) => ThemeFileKind::EzaMap,
        ThemeDocument::Map(ThemeMap::Gtk(_)) => ThemeFileKind::GtkMap,
        ThemeDocument::Map(ThemeMap::Kde(_)) => ThemeFileKind::KdeMap,
        ThemeDocument::Map(ThemeMap::Obsidian(_)) => ThemeFileKind::ObsidianMap,
    }
}

impl From<LoweringError> for ThemeLoweringError {
    fn from(error: LoweringError) -> Self {
        match error {
            LoweringError::MismatchedParse => Self::MismatchedParse,
        }
    }
}

#[cfg(test)]
mod sealing_tests {
    use super::*;

    #[test]
    fn sealing_rechecks_its_private_source_kind_schema_and_hir_snapshot() {
        let path = RepoPath::new("theme/roles.dotfile").unwrap();
        let source = SourceText::from("roles { foreground = blue }\n");
        let clean = lower_theme_file(&path, &source);
        assert!(clean.clone().into_validated().is_ok());

        let mut stale_source = clean.clone();
        stale_source.source = SourceText::from("roles { foreground = cyan }\n");
        assert!(stale_source.into_validated().is_err());

        let mut wrong_kind = clean.clone();
        wrong_kind.kind = Some(ThemeFileKind::Fonts);
        assert!(wrong_kind.into_validated().is_err());

        let fonts_path = RepoPath::new("theme/fonts.dotfile").unwrap();
        let fonts_source = SourceText::from(
            "fonts { general = \"Sans\", nerd = \"Nerd\" }\n\
             sizes { terminal = \"1\", terminal_mac = \"1\", interface = \"1\" }\n\
             applications {}\n",
        );
        let fonts = lower_theme_file(&fonts_path, &fonts_source);

        let mut wrong_schema = clean.clone();
        wrong_schema.schema = fonts.schema.clone();
        assert!(wrong_schema.into_validated().is_err());

        let mut wrong_hir = clean;
        wrong_hir.hir = fonts.hir.clone();
        assert!(wrong_hir.into_validated().is_err());
    }
}
