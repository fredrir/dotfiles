mod controls;
pub mod monitor;
mod search;
mod transaction;

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bench::{hosts, provenance, record, runner, store};
use crate::env::Sysfs;
use controls::{Control, Profile};
use search::Measurements;

#[derive(Subcommand)]
pub enum Command {
    /// Show supported OS controls and trial profiles without changing settings.
    Plan {
        #[arg(long)]
        json: bool,
    },
    /// Benchmark supported OS profiles and validate the best candidate.
    Auto(AutoOptions),
    /// Validate and apply this host's checked-out desired profile.
    Apply(ValidationOptions),
}

#[derive(Args)]
pub struct ValidationOptions {
    /// CPU metric required in each benchmark validation.
    #[arg(long, default_value = "cpu.multi")]
    pub metric: String,
    /// Abort above this Celsius temperature; default uses the sensor limit.
    #[arg(long)]
    pub max_temp: Option<f64>,
    /// Duration of each monitored CPU stability test.
    #[arg(long, default_value = "30", value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub stress_seconds: u64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct AutoOptions {
    /// Write the validated winner to the git-tracked profile and retain it live.
    #[arg(long)]
    pub apply: bool,
    /// Minimum improvement percentage, in addition to the measured noise band.
    #[arg(long, default_value = "3")]
    pub min_improvement: f64,
    #[command(flatten)]
    pub validation: ValidationOptions,
}

#[derive(Deserialize, Serialize)]
struct Desired {
    schema: u32,
    host: String,
    hardware_epoch: String,
    profile: Profile,
    drivers: std::collections::BTreeMap<PathBuf, Option<String>>,
    validated_session: String,
    validated_run: String,
}

#[derive(Serialize)]
struct Session {
    schema: u32,
    kind: &'static str,
    session: String,
    host: String,
    started: String,
    finished: Option<String>,
    objective: String,
    minimum_improvement_pct: Option<f64>,
    original: Profile,
    controls: Vec<Control>,
    trials: Vec<Value>,
    recommendation: Option<Profile>,
    status: String,
    error: Option<String>,
    retained: bool,
    restored: bool,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn local_host(explicit: Option<&str>) -> Result<String, String> {
    let environment = std::env::var("HWTUNE_HOST")
        .ok()
        .filter(|host| !host.is_empty());
    let explicit = explicit.or(environment.as_deref());
    let names = sysinfo::inventory::local_hostnames();
    let inventory = hosts::load_hosts()?;
    let matched = sysinfo::inventory::match_hostname(&inventory, &names);
    let local = if matched.is_empty() {
        names
            .first()
            .and_then(|name| name.split('.').next())
            .unwrap_or("")
            .to_owned()
    } else {
        matched
    };
    if !sysinfo::inventory::valid_name(&local) {
        return Err("cannot establish this machine's local host identity".into());
    }
    if explicit.is_some_and(|selected| selected != local) {
        return Err(format!(
            "OS tuning can only target the local machine {local:?}"
        ));
    }
    Ok(local)
}

fn desired_path(host: &str) -> Result<PathBuf, String> {
    if !sysinfo::inventory::valid_name(host) {
        return Err("invalid host name for desired tuning profile".into());
    }
    Ok(sysinfo::inventory::repo_root()
        .join("config/hwtune")
        .join(format!("{host}.json")))
}

fn read_optional(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn commit_profile(
    path: &std::path::Path,
    previous: Option<&[u8]>,
    next: &[u8],
) -> Result<(), String> {
    unchanged_profile(path, previous)?;
    if runner::cancelled() {
        return Err("tuning was interrupted before saving the desired profile".into());
    }
    store::atomic_write(path, next)
}

fn unchanged_profile(path: &std::path::Path, expected: Option<&[u8]>) -> Result<(), String> {
    if read_optional(path)?.as_deref() != expected {
        Err("desired profile changed during tuning; preserving the edited file".into())
    } else {
        Ok(())
    }
}

fn undo_profile(
    path: &std::path::Path,
    previous: Option<&[u8]>,
    next: &[u8],
) -> Result<(), String> {
    let current = read_optional(path)?;
    if current.as_deref() == previous {
        return Ok(());
    }
    if current.as_deref() != Some(next) {
        return Err("desired profile changed outside tuning; preserving that edit".into());
    }
    if let Some(bytes) = previous {
        store::atomic_write(path, bytes)
    } else {
        fs::remove_file(path).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn validate_options(options: &ValidationOptions) -> Result<(), String> {
    if !options.metric.starts_with("cpu.") {
        return Err("OS tuning requires a CPU benchmark metric such as cpu.multi".into());
    }
    if options
        .max_temp
        .is_some_and(|value| !value.is_finite() || !(40.0..=110.0).contains(&value))
    {
        return Err("--max-temp must be a finite Celsius value between 40 and 110".into());
    }
    Ok(())
}

fn save_session(store: &store::Store, session: &Session) -> Result<PathBuf, String> {
    let _lock = store.exclusive()?;
    let path = store.tuning_path(&session.host, &session.session)?;
    store::atomic_write(
        &path,
        &serde_json::to_vec_pretty(session).map_err(|error| error.to_string())?,
    )?;
    Ok(path)
}

struct Experiment<'a> {
    sys: &'a Sysfs,
    store: &'a store::Store,
    options: &'a ValidationOptions,
    session: &'a mut Session,
}

impl Measurements for Experiment<'_> {
    fn benchmark(&mut self, phase: &str) -> Result<record::Run, String> {
        if runner::cancelled() {
            return Err("tuning was interrupted".into());
        }
        let monitor = monitor::Monitor::start(self.sys, self.options.max_temp)?;
        let mut context = provenance::current(&self.session.host);
        context.tuning_session = Some(self.session.session.clone());
        let options = runner::Options {
            host: self.session.host.clone(),
            tier: "quick".into(),
            families: vec!["cpu".into()],
            note: format!("{}: {phase}", self.session.session),
            context: Some(context),
            ..runner::Options::default()
        };
        let measured = runner::execute(&options, &mut |event, job, detail| {
            if !self.options.json {
                eprintln!("{phase}: {event} {job} {detail}");
            }
        });
        let monitored = monitor.finish();
        let mut run = measured?;
        match &monitored {
            Ok(evidence) => {
                run.conditions["tuning_monitor"] =
                    serde_json::to_value(evidence).map_err(|error| error.to_string())?;
                if !evidence.passed {
                    run.grade = "noisy".into();
                    run.gate_reasons.push(format!(
                        "tuning telemetry: {}",
                        evidence.reason.as_deref().unwrap_or("validation failed")
                    ));
                }
            }
            Err(error) => {
                run.grade = "noisy".into();
                run.gate_reasons.push(format!("tuning telemetry: {error}"));
            }
        }
        let path = {
            let _lock = self.store.exclusive()?;
            self.store.save_run(&run)?
        };
        self.record(json!({"phase":phase,"run_id":run.run_id,"run_path":path,"grade":run.grade,"monitor":run.conditions["tuning_monitor"]}))?;
        monitored?;
        search::complete(&run, &self.options.metric)?;
        Ok(run)
    }

    fn stress(&mut self, phase: &str) -> Result<(), String> {
        if runner::cancelled() {
            return Err("tuning was interrupted".into());
        }
        let evidence =
            monitor::run_stress(self.sys, self.options.stress_seconds, self.options.max_temp)?;
        self.record(json!({"phase":phase,"stability":evidence}))?;
        if !evidence.passed {
            return Err(format!(
                "stability validation failed: {}",
                evidence.reason.as_deref().unwrap_or("validation failed")
            ));
        }
        Ok(())
    }
    fn record(&mut self, evidence: Value) -> Result<(), String> {
        self.session.trials.push(evidence);
        save_session(self.store, self.session)?;
        Ok(())
    }
}

fn desired_from(
    host: &str,
    profile: Profile,
    controls: &[Control],
    session: &str,
    run: &record::Run,
) -> Desired {
    Desired {
        schema: 1,
        host: host.into(),
        hardware_epoch: run.epoch(),
        profile,
        drivers: controls
            .iter()
            .map(|control| (control.path.clone(), control.driver.clone()))
            .collect(),
        validated_session: session.into(),
        validated_run: run.run_id.clone(),
    }
}

fn validate_desired(
    desired: &Desired,
    host: &str,
    controls: &[Control],
    baseline: &record::Run,
) -> Result<(), String> {
    if desired.schema != 1 || desired.host != host || desired.hardware_epoch != baseline.epoch() {
        return Err(
            "desired profile belongs to a different host, hardware configuration, or schema".into(),
        );
    }
    let drivers = controls
        .iter()
        .map(|control| (control.path.clone(), control.driver.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    if drivers != desired.drivers {
        return Err(
            "desired profile CPU policies or scaling drivers differ from the current machine"
                .into(),
        );
    }
    transaction::validate_profile(controls, &desired.profile)
}

fn execute(
    host: &str,
    sys: &Sysfs,
    options: &ValidationOptions,
    automatic: Option<(bool, f64)>,
) -> Result<ExitCode, String> {
    validate_options(options)?;
    let _signals = ui_terminal::SignalGuard::new().map_err(|error| error.to_string())?;
    let _measurement = store::measurement_lock()?;
    let plan = controls::discover(sys)?;
    if plan.controls.is_empty() {
        return Err(plan.unavailable.join("; "));
    }
    if automatic.is_some() && plan.candidates.is_empty() {
        return Err("no alternative restorable OS profiles are available".into());
    }
    let path = desired_path(host)?;
    let previous_profile = read_optional(&path)?;
    let desired =
        if automatic.is_none() {
            Some(
                serde_json::from_slice::<Desired>(previous_profile.as_deref().ok_or_else(
                    || format!("{}: desired profile does not exist", path.display()),
                )?)
                .map_err(|error| format!("{}: {error}", path.display()))?,
            )
        } else {
            None
        };
    let store = store::Store::discover();
    let mut session = Session {
        schema: 1,
        kind: "os-tuning",
        session: format!(
            "tune-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
            std::process::id()
        ),
        host: host.into(),
        started: now(),
        finished: None,
        objective: options.metric.clone(),
        minimum_improvement_pct: automatic.map(|(_, minimum)| minimum),
        original: controls::original_profile(&plan.controls),
        controls: plan.controls.clone(),
        trials: Vec::new(),
        recommendation: None,
        status: "running".into(),
        error: None,
        retained: false,
        restored: false,
    };
    save_session(&store, &session)?;
    let mut guard = transaction::Guard::begin(&sys.sys, plan.controls.clone())?;
    let mut pending_profile = None;
    let mut outcome = (|| {
        let mut experiment = Experiment {
            sys,
            store: &store,
            options,
            session: &mut session,
        };
        let (profile, validated) = if let Some((_, minimum)) = automatic {
            search::optimize(
                &mut experiment,
                &mut guard,
                &plan.candidates,
                &options.metric,
                minimum,
            )?
        } else {
            let desired = desired.as_ref().ok_or("missing desired profile")?;
            let baseline = experiment.benchmark("apply-baseline")?;
            validate_desired(desired, host, &plan.controls, &baseline)?;
            guard.apply(&desired.profile)?;
            experiment.stress("apply-stability")?;
            let validated = experiment.benchmark("apply-validation")?;
            // Applying a reverted profile may intentionally lower performance.
            // It still needs complete measurements and stability evidence.
            (desired.profile.clone(), validated)
        };
        experiment.session.recommendation = Some(profile.clone());
        if automatic.is_some_and(|(apply, _)| apply) {
            let desired = desired_from(
                host,
                profile,
                &plan.controls,
                &experiment.session.session,
                &validated,
            );
            pending_profile =
                Some(serde_json::to_vec_pretty(&desired).map_err(|error| error.to_string())?);
        }
        Ok(())
    })();
    if outcome.is_ok() && runner::cancelled() {
        outcome = Err("tuning was interrupted before committing settings".into());
    }
    if outcome.is_ok() && automatic.is_none() {
        outcome = unchanged_profile(&path, previous_profile.as_deref());
    }
    if outcome.is_ok()
        && let Some(bytes) = &pending_profile
    {
        outcome = commit_profile(&path, previous_profile.as_deref(), bytes);
    }
    let retain = outcome.is_ok() && automatic.is_none_or(|(apply, _)| apply);
    let finalized = guard.complete(retain);
    session.retained = retain && finalized.is_ok();
    let restored = if retain && finalized.is_err() {
        guard.complete(false)
    } else if retain {
        Ok(())
    } else {
        finalized.clone()
    };
    session.restored = !session.retained && restored.is_ok();
    if let Err(error) = finalized {
        outcome = Err(match outcome {
            Ok(()) => error,
            Err(previous) => format!("{previous}; {error}"),
        });
    }
    if outcome.is_err()
        && let Some(bytes) = &pending_profile
        && let Err(error) = undo_profile(&path, previous_profile.as_deref(), bytes)
    {
        outcome = Err(format!(
            "{}; could not restore previous desired profile: {error}",
            outcome.unwrap_err()
        ));
    }
    session.finished = Some(now());
    let result = match (outcome, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(restore)) => Err(format!(
            "{error}; original settings could not be fully restored: {restore}"
        )),
    };
    session.status = if result.is_ok() {
        "validated"
    } else if runner::cancelled() {
        "aborted"
    } else {
        "failed"
    }
    .into();
    session.error = result.as_ref().err().cloned();
    let record_path = save_session(&store, &session).map_err(|error| {
        format!(
            "{error}; live settings {}",
            if session.retained {
                "were retained and match the desired profile"
            } else if session.restored {
                "were restored to their starting values"
            } else {
                "could not be fully restored"
            }
        )
    })?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&session).map_err(|error| error.to_string())?
        );
    } else {
        println!("Tuning session: {}", record_path.display());
        if let Some(profile) = &session.recommendation {
            println!(
                "Validated profile: {} ({})",
                profile.name,
                if session.retained {
                    "applied"
                } else if session.restored {
                    "original settings restored"
                } else {
                    "restoration incomplete"
                }
            );
            if session.retained {
                println!("Desired settings: {}", path.display());
            }
        }
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

pub fn run(command: Command, host: Option<&str>, sys: &Sysfs) -> Result<ExitCode, String> {
    match command {
        Command::Plan { json } => {
            let plan = controls::discover(sys)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).map_err(|error| error.to_string())?
                );
            } else {
                for control in &plan.controls {
                    println!(
                        "{} = {} (available: {})",
                        control.path.display(),
                        control.original,
                        control.choices.join(", ")
                    );
                }
                for candidate in &plan.candidates {
                    println!("Trial profile: {}", candidate.name);
                }
                for reason in &plan.unavailable {
                    println!("Unavailable: {reason}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Auto(options) => {
            if !options.min_improvement.is_finite() || options.min_improvement < 0.0 {
                return Err("--min-improvement must be a finite nonnegative percentage".into());
            }
            execute(
                &local_host(host)?,
                sys,
                &options.validation,
                Some((options.apply, options.min_improvement)),
            )
        }
        Command::Apply(options) => execute(&local_host(host)?, sys, &options, None),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tune/cli_tests.rs"]
mod tests;
