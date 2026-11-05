use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dotfmt_core::config::{Effective, Selection};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use dotfmt_markdown::dialect::Dialect;

use super::engine::Engine;
use super::ignore::ScanPolicy;
use super::{
    Buffer, Explanation, Operation, Request, Session, attach_path, configuration, discovery,
    forced, ordered, requested,
};

pub struct Prepared {
    request: Request,
    configs: Vec<Arc<Effective>>,
    files: Vec<Selected>,
    buffer: Option<SelectedBuffer>,
    owned: Vec<PathBuf>,
    unreadable: usize,
    parallel: bool,
}

struct Selected {
    path: PathBuf,
    absolute: PathBuf,
    language: Language,
    config: Arc<Effective>,
}

struct SelectedBuffer {
    input: Buffer,
    absolute: PathBuf,
    language: Option<Language>,
    config: Arc<Effective>,
}

pub struct Plan {
    pub(super) work: Work,
    pub(super) parallel: bool,
}

pub(super) enum Work {
    Format {
        files: Vec<Job>,
        buffer: Option<BufferJob>,
        unreadable: usize,
        check: bool,
        clean: bool,
        fixes: bool,
    },
    Owns(Vec<PathBuf>),
    Inspect {
        files: Vec<Job>,
        buffer: Option<BufferJob>,
        unreadable: usize,
        fix: super::FixMode,
        diff: bool,
        json: bool,
        type_aware: Option<bool>,
        project_analysis: bool,
    },
}

pub(super) struct Job {
    pub path: PathBuf,
    pub absolute: PathBuf,
    pub language: Language,
    pub engine: Arc<Engine>,
    pub markdown: Dialect,
}

pub(super) struct BufferJob {
    pub input: Buffer,
    pub absolute: PathBuf,
    pub language: Option<Language>,
    pub engine: Option<Arc<Engine>>,
    pub markdown: Dialect,
}

impl Session {
    pub fn prepare(&mut self, mut request: Request) -> Result<Prepared, Vec<Diagnostic>> {
        request.languages.sort();
        request.languages.dedup();
        let mut errors = Vec::new();
        let mut configs = HashMap::new();
        let mut files = Vec::new();
        let mut buffer = None;
        let mut owned = Vec::new();
        let mut unreadable = 0;
        let mut parallel = false;
        let input = match &mut request.operation {
            Operation::Format { buffer, .. } | Operation::Inspect { buffer, .. } => buffer.take(),
            Operation::Owns { .. } => None,
        };
        match &request.operation {
            Operation::Owns { paths } => {
                if request.dialect.is_some() {
                    return Err(vec![configuration(
                        "dialect overrides do not apply to ownership queries",
                    )]);
                }
                let scan = ScanPolicy::current();
                for path in paths {
                    let absolute = match self.resolver.absolute_path(path) {
                        Ok(absolute) => absolute,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
                    let config = match self.resolver.for_file(path) {
                        Ok(config) => config,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
                    if !scan.allows(&absolute) {
                        continue;
                    }
                    configs.insert(Arc::as_ptr(&config) as usize, Arc::clone(&config));
                    match config.select_languages(path, &request.languages, None) {
                        Ok(Some(language)) if requested(&request, language) => {
                            owned.push(path.clone())
                        }
                        Ok(_) => {}
                        Err(error) => errors.push(error),
                    }
                }
            }
            Operation::Format { targets, .. } | Operation::Inspect { targets, .. } => {
                if let Some(input) = input {
                    match self.select_buffer(&request, input) {
                        Ok(selected) => {
                            configs.insert(
                                Arc::as_ptr(&selected.config) as usize,
                                Arc::clone(&selected.config),
                            );
                            buffer = Some(selected);
                        }
                        Err(error) => errors.push(error),
                    }
                }
                if !targets.is_empty() {
                    parallel = targets.len() > 1
                        || targets.iter().any(|path| {
                            self.resolver
                                .absolute_path(path)
                                .is_ok_and(|path| path.is_dir())
                        });
                    let found = if parallel {
                        self.pool()
                            .map_err(|error| vec![error])?
                            .install(|| discovery::discover(targets, &self.resolver))
                    } else {
                        discovery::discover(targets, &self.resolver)
                    };
                    unreadable = found.unreadable;
                    errors.extend(found.diagnostics);
                    let mut configured = BTreeSet::new();
                    for directory in found.directories {
                        match self.resolver.for_directory(&directory) {
                            Ok(config) => {
                                enabled(&config, &mut configured);
                                configs.insert(Arc::as_ptr(&config) as usize, config);
                            }
                            Err(error) => errors.push(error),
                        }
                    }
                    let mut visited = HashSet::new();
                    for candidate in found.candidates {
                        if !candidate.absolute.is_file() {
                            continue;
                        }
                        let config = match self.resolver.for_file(&candidate.path) {
                            Ok(config) => config,
                            Err(error) => {
                                errors.push(error);
                                continue;
                            }
                        };
                        enabled(&config, &mut configured);
                        configs.insert(Arc::as_ptr(&config) as usize, Arc::clone(&config));
                        if candidate.explicit
                            && let Err(error) = ensure_requested(&request, &config)
                        {
                            errors.push(attach_path(error, &candidate.path));
                            continue;
                        }
                        let selection = match config.selection(
                            &candidate.path,
                            &request.languages,
                            if candidate.explicit {
                                forced(&request)
                            } else {
                                None
                            },
                        ) {
                            Ok(selection) => selection,
                            Err(error) => {
                                errors.push(error);
                                continue;
                            }
                        };
                        let language = match selection {
                            Selection::Language(language) if requested(&request, language) => {
                                language
                            }
                            Selection::Excluded | Selection::Unmapped if !candidate.explicit => {
                                continue;
                            }
                            Selection::Excluded => continue,
                            Selection::Disabled(language) if candidate.explicit => {
                                errors.push(
                                    Diagnostic::new(
                                        DiagnosticKind::Configuration,
                                        format!(
                                            "{} is disabled for {}",
                                            language.name(),
                                            candidate.path.display()
                                        ),
                                    )
                                    .with_path(&candidate.path),
                                );
                                continue;
                            }
                            Selection::Unmapped if candidate.explicit => {
                                errors.push(
                                    Diagnostic::new(
                                        DiagnosticKind::Configuration,
                                        format!(
                                            "no configured language for {}",
                                            candidate.path.display()
                                        ),
                                    )
                                    .with_path(&candidate.path),
                                );
                                continue;
                            }
                            _ => continue,
                        };
                        if !ScanPolicy::at(&candidate.root).allows(&candidate.absolute) {
                            continue;
                        }
                        let canonical = match fs::canonicalize(&candidate.absolute) {
                            Ok(path) => path,
                            Err(error) => {
                                errors.push(
                                    Diagnostic::new(DiagnosticKind::Io, error.to_string())
                                        .with_path(&candidate.path),
                                );
                                continue;
                            }
                        };
                        if visited.insert(canonical) {
                            files.push(Selected {
                                path: candidate.path,
                                absolute: candidate.absolute,
                                language,
                                config,
                            });
                        }
                    }
                    for language in &request.languages {
                        if !configured.contains(language) {
                            errors.push(unconfigured(*language));
                        }
                    }
                    if configured.is_empty() && errors.is_empty() {
                        errors.push(no_languages());
                    }
                }
            }
        }
        if !errors.is_empty() {
            return Err(ordered(errors));
        }
        Ok(Prepared {
            request,
            configs: configs.into_values().collect(),
            files,
            buffer,
            owned,
            unreadable,
            parallel,
        })
    }

    /// Resolves what editor mode would do with `path` without reading or formatting it.
    pub fn explain(
        &mut self,
        path: &Path,
        mut languages: Vec<Language>,
        dialect: Option<String>,
    ) -> Result<Explanation, Vec<Diagnostic>> {
        languages.sort();
        languages.dedup();
        let request = &Request {
            operation: Operation::Owns {
                paths: vec![path.to_path_buf()],
            },
            languages,
            dialect,
            clean: false,
        };
        let (config, selection) = self
            .resolve_buffer(request, path)
            .map_err(|error| vec![error])?;
        if ScanPolicy::at(path).ignored(path) {
            return Ok(Explanation {
                selection: Selection::Excluded,
                dialect: None,
                sources: config.sources().to_vec(),
            });
        }
        self.engines
            .validate(&config)
            .map_err(|error| vec![error])?;
        ensure_dialect(
            request,
            selection
                .language()
                .into_iter()
                .chain(request.languages.iter().copied()),
        )
        .map_err(|error| vec![error])?;
        let dialect = match selection {
            Selection::Language(language) => {
                let engine = self
                    .engines
                    .get(&config, language, request.dialect.as_deref())
                    .map_err(|error| vec![attach_path(error, path)])?;
                let absolute = self
                    .resolver
                    .absolute_path(path)
                    .map_err(|error| vec![error])?;
                let markdown = if language == Language::Markdown {
                    self.context.markdown(&absolute)
                } else {
                    Dialect::Gfm
                };
                engine.dialect(path, &absolute, markdown)
            }
            _ => None,
        };
        Ok(Explanation {
            selection,
            dialect,
            sources: config.sources().to_vec(),
        })
    }

    fn select_buffer(
        &self,
        request: &Request,
        input: Buffer,
    ) -> Result<SelectedBuffer, Diagnostic> {
        let (config, selection) = self.resolve_buffer(request, &input.path)?;
        Ok(SelectedBuffer {
            absolute: self.resolver.absolute_path(&input.path)?,
            input,
            language: selection
                .language()
                .filter(|language| requested(request, *language)),
            config,
        })
    }

    fn resolve_buffer(
        &self,
        request: &Request,
        path: &Path,
    ) -> Result<(Arc<Effective>, Selection), Diagnostic> {
        let config = self.resolver.for_file(path)?;
        ensure_requested(request, &config)?;
        if !config
            .languages
            .iter()
            .any(|(language, config)| config.enabled && requested(request, *language))
        {
            return Err(no_languages());
        }
        let selection = config.selection(path, &request.languages, forced(request))?;
        Ok((config, selection))
    }

    pub fn validate(&mut self, prepared: Prepared) -> Result<Plan, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        for config in &prepared.configs {
            if let Err(error) = self.engines.validate(config) {
                errors.push(error);
            }
        }
        if !errors.is_empty() {
            return Err(ordered(errors));
        }
        let Prepared {
            request,
            files,
            buffer,
            owned,
            unreadable,
            parallel,
            ..
        } = prepared;
        if matches!(request.operation, Operation::Owns { .. }) {
            return Ok(Plan {
                work: Work::Owns(owned),
                parallel: false,
            });
        }
        ensure_dialect(
            &request,
            files
                .iter()
                .map(|file| file.language)
                .chain(buffer.iter().filter_map(|buffer| buffer.language))
                .chain(request.languages.iter().copied()),
        )
        .map_err(|error| vec![error])?;
        ensure_clean(&request).map_err(|error| vec![error])?;
        let mut jobs = Vec::new();
        for selected in files {
            match self.engines.get(
                &selected.config,
                selected.language,
                request.dialect.as_deref(),
            ) {
                Ok(engine) => {
                    let markdown = if selected.language == Language::Markdown {
                        self.context.markdown(&selected.absolute)
                    } else {
                        Dialect::Gfm
                    };
                    jobs.push(Job {
                        path: selected.path,
                        absolute: selected.absolute,
                        language: selected.language,
                        engine,
                        markdown,
                    });
                }
                Err(error) => errors.push(attach_path(error, &selected.path)),
            }
        }
        let buffer = buffer.and_then(|selected| {
            let engine = selected
                .language
                .map(|language| {
                    self.engines
                        .get(&selected.config, language, request.dialect.as_deref())
                })
                .transpose();
            match engine {
                Ok(engine) => {
                    let markdown = if selected.language == Some(Language::Markdown) {
                        self.context.markdown(&selected.absolute)
                    } else {
                        Dialect::Gfm
                    };
                    Some(BufferJob {
                        input: selected.input,
                        absolute: selected.absolute,
                        language: selected.language,
                        engine,
                        markdown,
                    })
                }
                Err(error) => {
                    errors.push(attach_path(error, &selected.input.path));
                    None
                }
            }
        });
        if !errors.is_empty() {
            return Err(ordered(errors));
        }
        match request.operation {
            Operation::Format { check, fixes, .. } => Ok(Plan {
                work: Work::Format {
                    files: jobs,
                    buffer,
                    unreadable,
                    check,
                    clean: request.clean,
                    fixes,
                },
                parallel,
            }),
            Operation::Inspect {
                fix,
                diff,
                json,
                type_aware,
                project_analysis,
                ..
            } => Ok(Plan {
                work: Work::Inspect {
                    files: jobs,
                    buffer,
                    unreadable,
                    fix,
                    diff,
                    json,
                    type_aware,
                    project_analysis,
                },
                parallel,
            }),
            Operation::Owns { .. } => unreachable!(),
        }
    }
}

fn enabled(config: &Effective, languages: &mut BTreeSet<Language>) {
    languages.extend(
        config
            .languages
            .iter()
            .filter(|(_, config)| config.enabled)
            .map(|(language, _)| *language),
    );
}

fn unconfigured(language: Language) -> Diagnostic {
    configuration(format!(
        "{} is not configured; add {} {{}} to dotfmt.dotfile or install a configuration with --add",
        language.name(),
        language.name()
    ))
}

fn no_languages() -> Diagnostic {
    configuration("no configured languages; add language blocks to dotfmt.dotfile or use --add")
}

fn ensure_requested(request: &Request, config: &Effective) -> Result<(), Diagnostic> {
    if let Some(language) = forced(request)
        && !config
            .languages
            .get(&language)
            .is_some_and(|config| config.enabled)
    {
        return Err(unconfigured(language));
    }
    Ok(())
}

fn ensure_dialect(
    request: &Request,
    languages: impl IntoIterator<Item = Language>,
) -> Result<(), Diagnostic> {
    if request
        .dialect
        .as_deref()
        .is_none_or(|value| value.eq_ignore_ascii_case("auto"))
    {
        return Ok(());
    }
    let unique: BTreeSet<_> = languages.into_iter().collect();
    if unique.len() != 1 {
        return Err(configuration(
            "--dialect requires one selected language; use --lang LANGUAGE",
        ));
    }
    Ok(())
}

fn ensure_clean(request: &Request) -> Result<(), Diagnostic> {
    if request.clean && request.languages != [Language::Markdown] {
        return Err(configuration(
            "--clean requires --lang markdown (or md); it formats Markdown and strips all formatting",
        ));
    }
    Ok(())
}
