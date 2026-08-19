//! `dmux repair reconcile`, black box (plan §10.2/§10.3, cases 11, 13, 39).
//!
//! The library half of crash reconciliation is unit-tested beside the code in
//! `src/operations.rs`; what has to be proven from outside is the operator
//! contract: the preview, §7.4's confirmation rule, and case 43's "exactly
//! one §16.2 document on every branch, refusals included".
//!
//! These drive the real binary against a scratch registry through the hidden
//! `--data-dir`/`--lock-dir` seams `repair normalize` already uses, so no
//! production path is consulted; XDG and the runtime dir are redirected too,
//! belt and braces.

use std::process::{Command, Output, Stdio};

use dmux::model::{Lifecycle, OperationKind, OperationState};
use dmux::registry::SpaceReservation;
use serde_json::Value;
use uuid::Uuid;

use super::util::{self, Scratch};

/// A `reserved` Space beside a `prepared` adopt row: exactly what a SIGKILL
/// between `reserve_space_kind` and `abort_create` leaves behind, and what no
/// verb could reap before `repair reconcile`.
fn stranded_adoption(scratch: &Scratch) -> SpaceReservation {
    let mut registry = util::open(&scratch.config);
    let instance = util::tmux_instance(&mut registry);
    registry
        .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap()
}

fn dmux(scratch: &Scratch, args: &[&str]) -> Output {
    let data_dir = scratch.dir.path().display().to_string();
    let lock_dir = scratch.config.lock_dir.display().to_string();
    let sink = scratch.dir.path().join("xdg");
    std::fs::create_dir_all(&sink).unwrap();
    Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args(args)
        .args(["--data-dir", &data_dir, "--lock-dir", &lock_dir])
        // The seams above are what the command reads; these only guarantee
        // that a mistake in them cannot reach the real registry.
        .env("XDG_DATA_HOME", &sink)
        .env("XDG_STATE_HOME", &sink)
        .env("XDG_RUNTIME_DIR", &sink)
        .env("DMUX_RUNTIME_DIR", &sink)
        .env("TMUX_TMPDIR", &sink)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("DMUX_HOST_UID")
        .env_remove("DMUX_SPACE_UID")
        .env_remove("WEZTERM_UNIX_SOCKET")
        .env_remove("WEZTERM_PANE")
        // Not a terminal, which is the state §7.4's rule is written for.
        .stdin(Stdio::null())
        .output()
        .expect("dmux runs")
}

/// Exactly one §16.2 document on stdout, and nothing else.
fn one_document(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim().lines().count(),
        1,
        "expected exactly one document, got: {stdout}"
    );
    serde_json::from_str(stdout.trim()).expect("stdout is one JSON document")
}

fn state_of(scratch: &Scratch, reservation: &SpaceReservation) -> (Lifecycle, OperationState) {
    let registry = util::open(&scratch.config);
    (
        registry.space(reservation.space_uid).unwrap().lifecycle,
        registry.operation(reservation.operation_uid).unwrap().state,
    )
}

#[test]
fn json_without_yes_previews_the_stranded_row_and_changes_nothing() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(&scratch, &["--format", "json", "repair", "reconcile"]);
    assert_eq!(output.status.code(), Some(5));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["action"], "repair_reconcile");
    assert_eq!(document["errors"][0]["code"], "confirmation_required");
    // §7.4: a JSON destructive verb never prompts, so the preview has to
    // travel inside the confirmation document or the operator never sees it.
    let target = &document["result"]["targets"][0];
    assert_eq!(target["kind"], "adopt");
    assert_eq!(target["state"], "prepared");
    assert_eq!(target["lifecycle"], "reserved");
    assert_eq!(target["duty"], "adoption_reconcile");
    assert_eq!(target["in_flight"], false);
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn yes_releases_the_burned_name_in_one_document() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["errors"].as_array().unwrap().len(), 0);
    assert_eq!(
        document["result"]["results"][0]["outcome"],
        "reservation_released"
    );
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Aborted, OperationState::Aborted)
    );

    // The whole point: the logical name is available again.
    let mut registry = util::open(&scratch.config);
    let instance = util::tmux_instance(&mut registry);
    registry
        .reserve_space("legacy", instance, Uuid::new_v4())
        .unwrap();
}

#[test]
fn a_second_run_finds_nothing_and_still_answers_with_a_document() {
    let scratch = util::scratch();
    stranded_adoption(&scratch);
    dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["result"]["targets"].as_array().unwrap().len(), 0);
    assert_eq!(document["result"]["results"].as_array().unwrap().len(), 0);
}

#[test]
fn a_ref_that_is_not_stranded_is_a_document_too_never_a_silent_empty_run() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(
        &scratch,
        &[
            "--format",
            "json",
            "repair",
            "reconcile",
            "--yes",
            "nosuchspace",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["action"], "repair_reconcile");
    assert_eq!(document["errors"][0]["code"], "not_found");
    assert_eq!(document["errors"][0]["target"], "nosuchspace");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_named_ref_reconciles_only_that_row() {
    let scratch = util::scratch();
    let first = stranded_adoption(&scratch);
    let second = {
        let mut registry = util::open(&scratch.config);
        let instance = util::tmux_instance(&mut registry);
        registry
            .reserve_space_kind("other", instance, Uuid::new_v4(), OperationKind::Adopt)
            .unwrap()
    };

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes", "legacy"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["result"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        state_of(&scratch, &first),
        (Lifecycle::Aborted, OperationState::Aborted)
    );
    assert_eq!(
        state_of(&scratch, &second),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_pipe_without_yes_changes_nothing_and_exits_five() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(&scratch, &["repair", "reconcile"]);
    assert_eq!(output.status.code(), Some(5));
    // The preview still prints: a human reading the pipe learns which rows
    // are stranded even though nothing was applied.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("legacy"), "{stdout}");
    assert!(stdout.contains("adoption_reconcile"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("confirmation required"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_row_a_live_holder_owns_is_reported_as_a_partial_never_touched() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);
    let owner = util::open(&scratch.config).identity().unwrap().host_uid;

    // Stand in for the still-running holder: the §10.1 locks a live adopt
    // keeps down for the whole of its call. The kernel drops them when a
    // holder dies, which is exactly what makes them the crash discriminator.
    let mut holder = dmux::locks::OrderedLocks::new(&scratch.config.lock_dir);
    holder
        .acquire(
            dmux::locks::LockScope::AuthorityGate,
            dmux::locks::LockMode::Shared,
        )
        .unwrap();
    holder
        .acquire_decisions(owner, &["legacy"], dmux::locks::LockMode::Exclusive)
        .unwrap();

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(7));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(
        document["result"]["results"][0]["outcome"],
        "skipped_in_flight"
    );
    assert_eq!(document["errors"][0]["code"], "operation_in_progress");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
    drop(holder);
}

#[test]
fn an_empty_registry_answers_with_a_document_rather_than_nothing() {
    let scratch = util::scratch();
    // Materialize the registry without stranding anything.
    let _ = util::open(&scratch.config);

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["result"]["targets"].as_array().unwrap().len(), 0);
}
