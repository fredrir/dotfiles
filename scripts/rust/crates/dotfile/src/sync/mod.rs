use std::time::Instant;

use crate::artifacts;
use crate::cli::SyncCli;
use crate::context::Context;
use crate::decision::Client;
use crate::event::{Event, EventSink, Summary};
use crate::lock::SyncLock;

pub fn run(cli: &SyncCli, events: &dyn EventSink, decisions: &Client) -> Result<Summary, String> {
    let started = Instant::now();
    let context = Context::discover()?;
    let profile = context.profile(cli.profile.as_deref())?;
    let peer = if cli.push || cli.to.is_some() {
        Some(crate::push::resolve_host(&context, cli.to.as_deref())?)
    } else {
        None
    };
    events.emit(Event::Started {
        profile: profile.clone(),
        dry_run: cli.dry_run,
        peer: peer.clone(),
    });
    events.emit(Event::PhaseStarted {
        phase: crate::event::Phase::Preflight,
        total: Some(if peer.is_some() { 3 } else { 1 }),
    });
    events.emit(Event::Progress {
        phase: crate::event::Phase::Preflight,
        completed: 0,
        total: Some(if peer.is_some() { 3 } else { 1 }),
        label: "waiting for the sync lock".to_string(),
    });
    let _lock = if cli.dry_run {
        None
    } else {
        Some(SyncLock::acquire(&context.state)?)
    };
    events.emit(Event::Progress {
        phase: crate::event::Phase::Preflight,
        completed: 1,
        total: Some(if peer.is_some() { 3 } else { 1 }),
        label: "sync lock ready".to_string(),
    });
    let push_plan = match peer.clone() {
        Some(host) => Some(crate::push::preflight_for_host(
            &context, cli, host, events,
        )?),
        None => None,
    };
    crate::cancel::check()?;
    let packages = artifacts::packages::synchronize(&context, cli.dry_run, events)?;
    crate::cancel::check()?;
    let docs = artifacts::docs::synchronize(&context, cli.dry_run, events)?;
    let generated = packages + docs;
    let mut summary = engine::reconcile(&context, &profile, cli, decisions, events)?;
    summary.generated += generated;
    summary.changed += generated;
    if let Some(plan) = push_plan {
        summary.remote_changed = crate::push::run_preflighted_with_decisions_summary(
            &context, cli, plan, events, decisions,
        )?;
        summary.peer = peer;
    }
    summary.elapsed = started.elapsed();
    events.emit(Event::Finished(summary.clone()));
    Ok(summary)
}

pub mod config;
pub mod engine;
pub mod integrations;
pub mod links;
pub mod merge;
pub mod secrets;
