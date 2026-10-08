use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use dotfmt_core::config::Effective;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use dotfmt_markdown::dialect::Dialect;

use super::engine::Engine;
use super::{
    Buffer, Operation, Request, Session, attach_path, configuration, discovery, forced, ordered,
    requested,
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
    },
    Owns(Vec<PathBuf>),
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
            Operation::Format { buffer, .. } => buffer.take(),
            Operation::Owns { .. } => None,
        };
        match &request.operation {
            Operation::Owns { paths } => {
                if request.dialect.is_some() {
                    return Err(vec![configuration(
                        "dialect overrides do not apply to ownership queries",
                    )]);
                }
                for path in paths {
                    let config = match self.resolver.for_file(path) {
                        Ok(config) => config,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
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
            Operation::Format { targets, .. } => {
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
                        let language = match config.select_languages(
                            &candidate.path,
                            &request.languages,
                            if candidate.explicit {
                                forced(&request)
                            } else {
                                None
                            },
                        ) {
                            Ok(Some(language)) if requested(&request, language) => language,
                            Ok(_) => continue,
                            Err(error) => {
                                errors.push(error);
                                continue;
                            }
                        };
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

    fn select_buffer(
        &self,
        request: &Request,
        input: Buffer,
    ) -> Result<SelectedBuffer, Diagnostic> {
        let config = self.resolver.for_file(&input.path)?;
        ensure_requested(request, &config)?;
        if !config
            .languages
            .iter()
            .any(|(language, config)| config.enabled && requested(request, *language))
        {
            return Err(no_languages());
        }
        let language = config
            .select_languages(&input.path, &request.languages, forced(request))?
            .filter(|language| requested(request, *language));
        Ok(SelectedBuffer {
            absolute: self.resolver.absolute_path(&input.path)?,
            input,
            language,
            config,
        })
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
        let Operation::Format { check, .. } = request.operation else {
            unreachable!()
        };
        Ok(Plan {
            work: Work::Format {
                files: jobs,
                buffer,
                unreadable,
                check,
            },
            parallel,
        })
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
