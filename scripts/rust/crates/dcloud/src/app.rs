use crate::cli::{Cli, Command, ConfigCommand, Filter, Repository};
use crate::config::{self, Config};
use crate::state::State;
use crate::{backups, catalog, objects, setup};
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

pub fn run(cli: Cli) -> std::result::Result<ExitCode, String> {
    execute(cli).map_err(|error| crate::ui::safe(&format!("{error:#}")))
}

fn execute(cli: Cli) -> Result<ExitCode> {
    let path = cli
        .config
        .clone()
        .map(Ok)
        .unwrap_or_else(config::default_path)?;
    let command = cli.command.unwrap_or(Command::Status {
        overdue: false,
        local: false,
    });
    let value = match command {
        Command::Init { host, secrets_dir } => {
            setup::initialize(&path, &host, secrets_dir.as_deref())?
        }
        Command::Config {
            command: ConfigCommand::Example,
        } => {
            print!("{}", toml::to_string_pretty(&setup::example("macie")?)?);
            return Ok(ExitCode::SUCCESS);
        }
        command => {
            let config = Config::load(&path)?;
            if let Command::Status { overdue, local } = command {
                let value = crate::status::collect(&config, local, overdue)?;
                emit(&value, cli.json)?;
                return Ok(if failed(&value) {
                    ExitCode::from(2)
                } else {
                    ExitCode::SUCCESS
                });
            }
            if matches!(
                command,
                Command::Config {
                    command: ConfigCommand::Check
                }
            ) {
                emit(
                    &json!({"config":path,"host":config.host,"valid":true}),
                    cli.json,
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            if let Command::AuthDrive {
                mut client,
                import_client,
                remote,
                authorize,
            } = command
            {
                if let Some(plaintext) = import_client {
                    let encrypted = config
                        .rclone_secrets_file
                        .as_deref()
                        .and_then(Path::parent)
                        .context("rclone_secrets_file must name config/dcloud/rclone.sops.json")?
                        .join("google-client.sops.json");
                    crate::secrets::seal_google_client(&config::expand(&plaintext)?, &encrypted)?;
                    client = Some(encrypted);
                }
                ensure!(
                    client.is_some() || authorize,
                    "use --import-client, --client or --authorize"
                );
                if let Some(client) = client {
                    crate::secrets::import_google_client(&config, &client, &remote)?;
                }
                if authorize {
                    crate::secrets::authorize_google(&config, &remote)?;
                }
                emit(&json!({"remote":remote,"authorized":authorize}), cli.json)?;
                return Ok(ExitCode::SUCCESS);
            }
            let runtime = needs_credentials(&command)
                .then(|| crate::secrets::materialize(&config))
                .transpose()?;
            let config = runtime
                .as_ref()
                .map(|runtime| runtime.config.clone())
                .unwrap_or(config);
            let mut state = State::open(&config.state_dir)?;
            let result = (|| -> Result<Value> {
                Ok(match command {
                    Command::Config {
                        command: ConfigCommand::Check,
                    } => json!({"config":path,"host":config.host,"valid":true}),
                    Command::Doctor { remote } => setup::doctor(&config, remote)?,
                    Command::Backup { job, all, dry_run } => {
                        ensure!(all != job.is_some(), "name one job or use --all");
                        let names = job.map(|name| vec![name]).unwrap_or_else(|| {
                            config
                                .jobs
                                .iter()
                                .filter(|(_, j)| j.sources.contains_key(&config.host))
                                .map(|(name, _)| name.clone())
                                .collect()
                        });
                        let mut results = Vec::new();
                        let mut errors = Vec::new();
                        for name in names {
                            match backups::backup(&config, &mut state, &name, dry_run) {
                                Ok(result) => results.push(result),
                                Err(error) => errors.push(format!("{name}: {error:#}")),
                            }
                        }
                        json!({"results":results,"errors":errors})
                    }
                    Command::Plan { job } => backups::backup(&config, &mut state, &job, true)?,
                    Command::Retry { run } => {
                        let mut errors = Vec::new();
                        let backups = match backups::retry(&config, &mut state, run.as_deref()) {
                            Ok(value) => value,
                            Err(error) => {
                                errors.push(format!("backups: {error:#}"));
                                Value::Null
                            }
                        };
                        let uploads = match objects::retry(&config, &mut state, run.as_deref()) {
                            Ok(value) => value,
                            Err(error) => {
                                errors.push(format!("uploads: {error:#}"));
                                Value::Null
                            }
                        };
                        json!({"backups":backups,"uploads":uploads,"errors":errors})
                    }
                    Command::RunDue { expect_host } => {
                        ensure!(
                            expect_host.as_ref().is_none_or(|host| host == &config.host),
                            "source host identity mismatch"
                        );
                        backups::run_due(&config, &mut state)?
                    }
                    Command::Dispatch { hosts } => dispatch(&config, &hosts)?,
                    Command::Upload {
                        path,
                        to,
                        category,
                        labels,
                        expires_days,
                        move_source,
                    } => objects::upload(
                        &config,
                        &mut state,
                        &path,
                        &to,
                        &category,
                        &labels,
                        expires_days,
                        move_source,
                    )?,
                    Command::Download {
                        id,
                        from,
                        host,
                        to,
                        paths,
                    } => serde_json::to_value(objects::download(
                        &config,
                        &id,
                        host.as_deref().unwrap_or(&config.host),
                        &from,
                        &config::expand(&to)?,
                        &paths,
                    )?)?,
                    Command::Browse {
                        filter,
                        offline,
                        tui,
                        snapshot,
                    } => browse(
                        &config,
                        &mut state,
                        &filter,
                        offline,
                        tui,
                        snapshot.as_deref(),
                    )?,
                    Command::Restore {
                        repository,
                        snapshot,
                        to,
                        paths,
                    } => {
                        let engine = repo(&config, &repository)?;
                        engine.restore(&snapshot, &config::expand(&to)?, &paths)?;
                        json!({"restored":snapshot,"to":to,"verified":true})
                    }
                    Command::Verify {
                        repository,
                        full,
                        subset,
                    } => {
                        repo(&config, &repository)?.check(full, subset.as_deref())?;
                        json!({"job":repository.job,"destination":repository.from,"verification":if full{"all stored data"}else if subset.is_some(){"stored data subset"}else{"repository structure"}})
                    }
                    Command::RestoreTest {
                        repository,
                        snapshot,
                    } => backups::restore_test(
                        &config,
                        &mut state,
                        repository.host.as_deref().unwrap_or(&config.host),
                        &repository.job,
                        &repository.from,
                        snapshot.as_deref(),
                    )?,
                    Command::Retention {
                        repository,
                        apply,
                        prune,
                    } => backups::retention(
                        &config,
                        &mut state,
                        repository.host.as_deref().unwrap_or(&config.host),
                        &repository.job,
                        &repository.from,
                        apply,
                        prune,
                    )?,
                    Command::Pin {
                        repository,
                        snapshot,
                        remove,
                    } => {
                        let _lock = state.lock(&format!(
                            "backup:{}:{}",
                            repository.host.as_deref().unwrap_or(&config.host),
                            repository.job
                        ))?;
                        let updated = repo(&config, &repository)?.pin(&snapshot, !remove)?;
                        json!({"snapshot":updated,"previous_snapshot":snapshot,"pinned":!remove})
                    }
                    Command::Label {
                        repository,
                        snapshot,
                        add,
                        remove,
                        category,
                    } => {
                        let _lock = state.lock(&format!(
                            "backup:{}:{}",
                            repository.host.as_deref().unwrap_or(&config.host),
                            repository.job
                        ))?;
                        let engine = repo(&config, &repository)?;
                        let existing = engine.snapshot(&snapshot)?;
                        let mut add = add
                            .iter()
                            .map(|v| format!("dcloud.label:{v}"))
                            .collect::<Vec<_>>();
                        let mut remove = remove
                            .iter()
                            .map(|v| format!("dcloud.label:{v}"))
                            .collect::<Vec<_>>();
                        if let Some(category) = category {
                            remove.extend(
                                existing
                                    .tags
                                    .iter()
                                    .filter(|v| v.starts_with("dcloud.category:"))
                                    .cloned(),
                            );
                            add.push(format!("dcloud.category:{category}"));
                        }
                        let updated = engine.set_tags(&snapshot, &add, &remove)?;
                        json!({"snapshot":updated,"previous_snapshot":snapshot,"added":add,"removed":remove})
                    }
                    Command::Diff {
                        repository,
                        older,
                        newer,
                    } => repo(&config, &repository)?.diff(&older, &newer)?,
                    Command::LabelUpload {
                        id,
                        from,
                        host,
                        add,
                        remove,
                        category,
                    } => objects::relabel(
                        &config,
                        &state,
                        &from,
                        host.as_deref().unwrap_or(&config.host),
                        &id,
                        category.as_deref(),
                        &add,
                        &remove,
                    )?,
                    Command::Stats {
                        repository,
                        forecast,
                    } => stats(&config, &repository, forecast)?,
                    Command::Repair {
                        repository,
                        snapshot,
                        to,
                    } => {
                        let host = repository.host.as_deref().unwrap_or(&config.host);
                        let _lock = state.lock(&format!("backup:{host}:{}", repository.job))?;
                        let donor = repo(&config, &repository)?;
                        donor.check(true, None)?;
                        let target = backups::repository(&config, host, &repository.job, &to)?;
                        target.init()?;
                        let id = target.copy_from(&donor, &snapshot)?;
                        ensure!(
                            target.snapshot(&id)?.tree == donor.snapshot(&snapshot)?.tree,
                            "replica tree mismatch"
                        );
                        target.check(false, None)?;
                        json!({"from":repository.from,"to":to,"snapshot":id,"donor_verified":true})
                    }
                    Command::Catalog { from } => catalog::collect(
                        &config,
                        &mut state,
                        &Filter {
                            from,
                            ..Filter::default()
                        },
                        false,
                    )?,
                    Command::Sync { pair, init, apply } => {
                        crate::sync::run(&config, &pair, init, apply)?
                    }
                    Command::Status { .. } => unreachable!(),
                    Command::Schedule { install, dispatch } => {
                        scheduler(&config, &path, install, dispatch)?
                    }
                    Command::Cleanup { apply } => cleanup(&config, &mut state, apply)?,
                    Command::RecoveryExport { to } => {
                        setup::export(&config, &config::expand(&to)?)?
                    }
                    Command::Init { .. }
                    | Command::AuthDrive { .. }
                    | Command::Config {
                        command: ConfigCommand::Example,
                    } => unreachable!(),
                })
            })();
            let persisted = runtime
                .as_ref()
                .map(|runtime| runtime.persist())
                .transpose();
            let result = result?;
            persisted?;
            result
        }
    };
    let failure = failed(&value);
    emit(&value, cli.json)?;
    Ok(if failure {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

fn needs_credentials(command: &Command) -> bool {
    !matches!(
        command,
        Command::Status { .. }
            | Command::Browse { offline: true, .. }
            | Command::Schedule { .. }
            | Command::Dispatch { .. }
            | Command::RecoveryExport { .. }
    )
}

fn failed(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (key == "errors" && value.as_array().is_some_and(|errors| !errors.is_empty()))
                || (key == "error" && value.as_str().is_some_and(|error| !error.is_empty()))
                || (key == "success" && value == &Value::Bool(false))
                || failed(value)
        }),
        Value::Array(values) => values.iter().any(failed),
        _ => false,
    }
}

fn repo(config: &Config, repository: &Repository) -> Result<crate::engine::Restic> {
    backups::repository(
        config,
        repository.host.as_deref().unwrap_or(&config.host),
        &repository.job,
        &repository.from,
    )
}

fn browse(
    config: &Config,
    state: &mut State,
    filter: &Filter,
    offline: bool,
    tui: bool,
    snapshot: Option<&str>,
) -> Result<Value> {
    if let Some(snapshot) = snapshot {
        ensure!(!offline, "file browsing requires repository access");
        let engine = backups::repository(
            config,
            filter.host.as_deref().unwrap_or(&config.host),
            filter.job.as_deref().context("--job required")?,
            filter.from.as_deref().context("--from required")?,
        )?;
        let entries = engine.ls(snapshot)?;
        if tui {
            return Ok(json!({"selected":crate::ui::tree(&entries)?}));
        }
        return Ok(json!({"files":entries}));
    }
    let result = catalog::collect(config, state, filter, offline)?;
    if tui {
        let rows = result["items"].as_array().context("catalog has no items")?;
        if let Some(index) = crate::ui::select(rows)? {
            let row = rows
                .get(index)
                .context("catalog selection is unavailable")?;
            let files = if row["kind"] == "archive" {
                row["manifest"]["entries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut entry| {
                        entry["type"] = entry["kind"].clone();
                        entry
                    })
                    .collect()
            } else {
                ensure!(
                    !offline,
                    "select a snapshot with online access to browse its files"
                );
                backups::repository(
                    config,
                    row["host"].as_str().context("host missing")?,
                    row["job"].as_str().context("job missing")?,
                    row["destination"].as_str().context("destination missing")?,
                )?
                .ls(row["id"].as_str().context("ID missing")?)?
            };
            return Ok(json!({"snapshot":row,"selected":crate::ui::tree(&files)?}));
        }
    }
    Ok(result)
}

fn stats(config: &Config, repository: &Repository, forecast: bool) -> Result<Value> {
    let engine = repo(config, repository)?;
    let stats = engine.stats(None)?;
    if !forecast {
        return Ok(stats);
    }
    let mut snapshots = engine.snapshots(
        Some(repository.host.as_deref().unwrap_or(&config.host)),
        Some(&repository.job),
    )?;
    snapshots.sort_by_key(|s| s.time);
    let total = stats["total_size"].as_u64().unwrap_or(0);
    let first = snapshots.first().map(|s| s.time);
    let span = first
        .map(|first| Utc::now().signed_duration_since(first).num_days().max(1))
        .unwrap_or(1);
    let daily = total as f64 / span as f64;
    let quota = config
        .destinations
        .get(&repository.from)
        .and_then(|d| d.quota_bytes);
    Ok(
        json!({"stats":stats,"projection":{"method":"average repository growth since first snapshot; ignores future retention","bytes_per_day":daily,"quota_bytes":quota,"days_to_quota":quota.filter(|q|*q>total).filter(|_|daily>0.).map(|q|(q-total)as f64/daily)}}),
    )
}

fn dispatch(config: &Config, hosts: &[String]) -> Result<Value> {
    let names = if hosts.is_empty() {
        config
            .hosts
            .keys()
            .filter(|h| *h != &config.host)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        hosts.to_vec()
    };
    let mut results = Vec::new();
    let mut errors = Vec::new();
    for name in names {
        config::identifier(&name)?;
        if name == config.host {
            continue;
        }
        let host = config
            .hosts
            .get(&name)
            .with_context(|| format!("unknown host: {name}"))?;
        let alias = host.ssh.as_deref().context("host has no SSH alias")?;
        let mut words = vec![
            host.binary.clone().unwrap_or_else(|| "dcloud".into()),
            "--json".into(),
        ];
        if let Some(path) = &host.config {
            words.extend(["--config".into(), path.clone()]);
        }
        words.extend(["run-due".into(), "--expect-host".into(), name.clone()]);
        let script = words
            .iter()
            .map(|s| hostkit::shell::quote(s))
            .collect::<Vec<_>>()
            .join(" ");
        let output = hostkit::ssh::Session::new(alias)
            .batch()
            .script(&script)
            .output_bounded(
                hostkit::process::CaptureLimits {
                    stdout: 8 * 1024 * 1024,
                    stderr: 64 * 1024,
                },
                Duration::from_secs(config.tools.timeout_seconds),
            );
        match output {
            Ok(output) if output.status.success() && !output.stdout_truncated=>results.push(json!({"host":name,"result":serde_json::from_slice::<Value>(&output.stdout).context("invalid scheduler response")?})),
            Ok(output)=>errors.push(format!("{name}: {}",String::from_utf8_lossy(&output.stderr).trim())),Err(error)=>errors.push(format!("{name}: {error}")),
        }
    }
    Ok(json!({"results":results,"errors":errors}))
}

fn scheduler(config: &Config, path: &Path, install: bool, dispatch: bool) -> Result<Value> {
    let executable = std::env::current_exe()?;
    let path = std::fs::canonicalize(path)?;
    let label = if dispatch {
        "io.dcloud.dispatch"
    } else {
        "io.dcloud.backup"
    };
    let mut files = if cfg!(target_os = "macos") {
        crate::schedule::launchd_with_logs(
            &executable,
            &path,
            label,
            &config.state_dir.join("logs"),
        )?
    } else {
        crate::schedule::systemd(&executable, &path, label)?
    };
    if dispatch {
        for file in &mut files {
            file.contents = file.contents.replace("run-due", "dispatch");
        }
    }
    let directory = if cfg!(target_os = "macos") {
        config::home()?.join("Library/LaunchAgents")
    } else {
        config::home()?.join(".config/systemd/user")
    };
    if install {
        setup::private_dir(&config.state_dir.join("logs"))?;
        setup::private_dir(&directory)?;
        for file in &files {
            let target = directory.join(&file.name);
            let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
            use std::io::Write;
            staged.write_all(file.contents.as_bytes())?;
            staged.as_file().sync_all()?;
            staged.persist(&target)?;
        }
        if cfg!(target_os = "macos") {
            let uid = setup::execute(
                std::process::Command::new("id").arg("-u"),
                Duration::from_secs(5),
            )?;
            let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());
            let target = directory.join(&files[0].name);
            let _ = setup::execute(
                std::process::Command::new("launchctl")
                    .args(["bootout", &format!("{domain}/{label}")]),
                Duration::from_secs(10),
            );
            setup::execute(
                std::process::Command::new("launchctl")
                    .arg("bootstrap")
                    .arg(&domain)
                    .arg(target),
                Duration::from_secs(10),
            )?;
        } else {
            setup::execute(
                std::process::Command::new("systemctl").args(["--user", "daemon-reload"]),
                Duration::from_secs(10),
            )?;
            setup::execute(
                std::process::Command::new("systemctl").args([
                    "--user",
                    "enable",
                    "--now",
                    &format!("{label}.timer"),
                ]),
                Duration::from_secs(10),
            )?;
        }
    }
    Ok(json!({"installed":install,"directory":directory,"files":files,"host":config.host}))
}

fn cleanup(config: &Config, state: &mut State, apply: bool) -> Result<Value> {
    crate::maintenance::cleanup(config, state, apply)
}

fn emit(value: &Value, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        print!("{}", crate::ui::human(value));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/app_tests.rs"]
mod tests;
