use crate::archive::{self, ArchiveOptions, Manifest, MetadataChange};
use crate::config::{self, Cleanup, Config, DestinationKind};
use crate::policy::{self, CleanupEvidence};
use crate::state::{ReplicaReceipt, ReplicaState, RunRecord, RunState, State};
use crate::transport::{ObjectInfo, Store};
use age::secrecy::{ExposeSecret, SecretString};
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_METADATA_REVISIONS: usize = 1024;
const MAX_METADATA_HISTORY_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct UploadPlan {
    source: PathBuf,
    fingerprint: String,
    targets: Vec<String>,
    variants: BTreeMap<String, PathBuf>,
    cleanup: Option<Cleanup>,
    config_hash: String,
    options: ArchiveOptions,
    prepared: bool,
}

pub fn identities(config: &Config) -> Result<Vec<String>> {
    if !config.identity_file.try_exists()? {
        return Ok(Vec::new());
    }
    crate::setup::check_secret(&config.identity_file)?;
    Ok(fs::read_to_string(&config.identity_file)?
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn store(config: &Config, name: &str) -> Result<Store> {
    let destination = config
        .destinations
        .get(name)
        .with_context(|| format!("unknown destination: {name}"))?;
    Ok(Store::new(
        destination,
        &config.tools.rclone,
        Duration::from_secs(config.tools.timeout_seconds),
    )?
    .with_rclone_config(config.rclone_config_file.clone()))
}

pub fn key(host: &str, id: &str) -> Result<String> {
    config::identifier(host)?;
    uuid::Uuid::parse_str(id).context("invalid archive ID")?;
    Ok(format!("{host}/uploads/{id}.dcloud"))
}

pub fn fingerprint(source: &Path) -> Result<String> {
    archive::tree_digest(source)
}

#[allow(clippy::too_many_arguments)]
pub fn upload(
    config: &Config,
    state: &mut State,
    source: &Path,
    targets: &[String],
    category: &str,
    labels: &[String],
    expires_days: Option<u32>,
    move_source: bool,
) -> Result<Value> {
    ensure!(!targets.is_empty(), "select an upload destination");
    let source = config::expand(source)?;
    ensure!(
        !fs::symlink_metadata(&source)?.file_type().is_symlink(),
        "upload source must not be a symlink"
    );
    let source = fs::canonicalize(&source).context("upload source not found")?;
    ensure!(
        source.parent().is_some() && source != fs::canonicalize(config::home()?)?,
        "select a source below the filesystem root or home"
    );
    ensure!(
        !config::overlap(&source, &config.state_dir)
            && !config::overlap(&source, &config.password_file)
            && !config::overlap(&source, &config.identity_file)
            && !config
                .rclone_config_file
                .as_ref()
                .is_some_and(|path| config::overlap(&source, path)),
        "source overlaps dcloud state or credentials"
    );
    let unique: std::collections::BTreeSet<_> = targets.iter().collect();
    ensure!(
        unique.len() == targets.len(),
        "duplicate upload destination"
    );
    for name in targets {
        let d = config
            .destinations
            .get(name)
            .with_context(|| format!("unknown destination: {name}"))?;
        ensure!(
            d.kind != DestinationKind::Rest,
            "{name}: REST repositories accept snapshots; use dcloud backup"
        );
        if d.kind == DestinationKind::Local {
            ensure!(
                !config::overlap(&source, &config::expand(Path::new(&d.location))?),
                "upload source overlaps destination"
            );
        }
        if d.encrypted {
            ensure!(
                !config.recipients.is_empty(),
                "{name}: no encryption recipient"
            );
        }
    }
    if move_source {
        ensure!(
            expires_days.is_none_or(|days| days > 14),
            "archive expiry must exceed the source quarantine period"
        );
        for pair in config
            .sync
            .values()
            .filter(|pair| pair.owner == config.host)
        {
            for side in [&pair.left, &pair.right] {
                if !side.contains(':') {
                    ensure!(
                        !config::overlap(&source, &config::expand(Path::new(side))?),
                        "source deletion overlaps a bidirectional sync root"
                    );
                }
            }
        }
    }
    if let Some(days) = expires_days {
        ensure!(days > 0 && days <= 36500, "expires_days must be 1..36500");
    }
    let _lock = state.lock(&format!("upload-source:{}", source.display()))?;
    let mut run = RunRecord::new(&config.host, "_uploads", &config.upload_digest(targets)?);
    let _run_lock = state.lock(&format!("upload:{}", run.id))?;
    state.save_run(&run)?;
    let stage = config.state_dir.join("uploads").join(&run.id);
    crate::setup::private_dir(&stage)?;
    let fingerprint = fingerprint(&source)?;
    let mut variants = BTreeMap::new();
    for name in targets {
        let encrypted = config.destinations[name].encrypted;
        variants.insert(
            name.clone(),
            stage.join(if encrypted {
                "encrypted.dcloud"
            } else {
                "plain.dcloud"
            }),
        );
    }
    let mut plan = UploadPlan {
        source,
        fingerprint,
        targets: targets.to_vec(),
        variants,
        cleanup: move_source.then_some(Cleanup {
            min_age_days: 1,
            quarantine_days: 14,
            min_verified_copies: targets.len(),
        }),
        config_hash: config.upload_digest(targets)?,
        options: ArchiveOptions {
            id: Some(run.id.clone()),
            host: config.host.clone(),
            job: "_uploads".into(),
            category: category.into(),
            labels: labels.to_vec(),
            expires_at: expires_days
                .map(|days| Utc::now() + chrono::Duration::days(i64::from(days))),
            compression_level: config.uploads.compression_level,
            compression_threads: config.uploads.compression_threads,
            ..ArchiveOptions::default()
        },
        prepared: false,
    };
    state.save_value("upload-plan", &run.id, &plan)?;
    run.state = RunState::BackingUp;
    state.save_run(&run)?;
    let result = prepare(config, state, &mut run, &mut plan)
        .and_then(|_| finish(config, state, &mut run, &plan));
    if let Err(error) = &result {
        if run.state != RunState::Committed {
            run.state = RunState::Failed;
        }
        run.error = Some(format!("{error:#}"));
        state.save_run(&run)?;
    }
    result
}

fn prepare(
    config: &Config,
    state: &mut State,
    run: &mut RunRecord,
    plan: &mut UploadPlan,
) -> Result<()> {
    validate_plan(config, run, plan)?;
    ensure!(
        plan.config_hash == config.upload_digest(&plan.targets)?,
        "upload configuration changed; restore previous configuration before retry"
    );
    if plan.prepared {
        if run.snapshot.is_none() {
            run.snapshot = Some(run.id.clone());
            run.state = RunState::Replicating;
            state.save_run(run)?;
        }
        return Ok(());
    }
    let _lock = state.lock("upload-spool")?;
    ensure!(
        fingerprint(&plan.source)? == plan.fingerprint,
        "upload source changed before packaging completed; start a new upload"
    );
    let keys = identities(config)?;
    let mut handled = std::collections::BTreeSet::new();
    for (name, output) in &plan.variants {
        if !handled.insert(output.clone()) {
            continue;
        }
        let encrypted = config.destinations[name].encrypted;
        let expected = config
            .state_dir
            .join("uploads")
            .join(&run.id)
            .join(if encrypted {
                "encrypted.dcloud"
            } else {
                "plain.dcloud"
            });
        ensure!(
            output == &expected,
            "upload spool path does not match its run"
        );
        if output.try_exists()? && archive::manifest_path(output).try_exists()? {
            let manifest = archive::verify(output, &keys)?;
            ensure!(
                manifest.id == run.id
                    && manifest.host == run.host
                    && manifest.encrypted == encrypted,
                "upload spool identity mismatch"
            );
            prepare_anchor(config, output, &manifest, &keys)?;
            continue;
        }
        for partial in [
            output.clone(),
            archive::manifest_path(output),
            anchor_path(output),
        ] {
            if let Ok(metadata) = fs::symlink_metadata(&partial) {
                ensure!(
                    metadata.is_file(),
                    "incomplete upload spool is not a regular file"
                );
                fs::remove_file(&partial)?;
            }
        }
        let used = spool_size(&config.state_dir.join("uploads"))?;
        let remaining = config
            .uploads
            .spool_limit_bytes
            .checked_sub(used)
            .context("pending upload spool exceeds its configured limit")?;
        let available = fs2::available_space(&config.state_dir)?.saturating_sub(64 * 1024 * 1024);
        let mut options = plan.options.clone();
        options.recipients = if encrypted {
            config.recipients.clone()
        } else {
            Vec::new()
        };
        options.max_output_bytes = remaining
            .min(available)
            .saturating_sub(archive::MAX_METADATA_BYTES);
        let manifest = archive::create(&plan.source, output, &options)?;
        archive::verify(output, &keys)?;
        prepare_anchor(config, output, &manifest, &keys)?;
        ensure!(
            fingerprint(&plan.source)? == plan.fingerprint,
            "source changed while packaging upload"
        );
        state.cache_manifest("spool", &run.id, &manifest)?;
    }
    plan.prepared = true;
    state.save_value("upload-plan", &run.id, plan)?;
    run.snapshot = Some(run.id.clone());
    run.state = RunState::Replicating;
    state.save_run(run)?;
    Ok(())
}

fn anchor_path(archive: &Path) -> PathBuf {
    let mut path = archive.as_os_str().to_os_string();
    path.push(".anchor");
    PathBuf::from(path)
}

fn prepare_anchor(
    config: &Config,
    output: &Path,
    manifest: &Manifest,
    identities: &[String],
) -> Result<()> {
    let path = anchor_path(output);
    let authentication_key = metadata_key(config)?;
    if !path.try_exists()? {
        let recipients = if manifest.encrypted {
            &config.recipients[..]
        } else {
            &[]
        };
        archive::write_archive_anchor(
            &path,
            manifest,
            recipients,
            authentication_key.expose_secret().as_bytes(),
        )?;
    }
    archive::verify_archive_anchor(
        &path,
        manifest,
        identities,
        authentication_key.expose_secret().as_bytes(),
    )?;
    ensure!(
        spool_size(&config.state_dir.join("uploads"))? <= config.uploads.spool_limit_bytes,
        "archive authentication record exceeds configured spool limit"
    );
    Ok(())
}

fn spool_size(root: &Path) -> Result<u64> {
    if !root.try_exists()? {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "upload spool contains a symlink"
        );
        if metadata.is_file() {
            total = total
                .checked_add(metadata.len())
                .context("upload spool size overflow")?;
        }
    }
    Ok(total)
}

fn finish(
    config: &Config,
    state: &mut State,
    run: &mut RunRecord,
    plan: &UploadPlan,
) -> Result<Value> {
    validate_plan(config, run, plan)?;
    ensure!(
        plan.config_hash == config.upload_digest(&plan.targets)?,
        "upload configuration changed; restore previous configuration before retry"
    );
    let object_key = key(&run.host, &run.id)?;
    for name in &plan.targets {
        if run
            .replicas
            .get(name)
            .is_some_and(|r| r.state == ReplicaState::Verified)
        {
            continue;
        }
        let mut receipt = ReplicaReceipt {
            destination: name.clone(),
            snapshot: Some(run.id.clone()),
            offsite: config.destinations[name].offsite,
            state: ReplicaState::Pending,
            verified_at: None,
            full_verified_at: None,
            error: None,
        };
        run.replicas.insert(name.clone(), receipt.clone());
        state.save_run(run)?;
        let result = (|| -> Result<()> {
            let store = store(config, name)?;
            let archive = &plan.variants[name];
            let keys = identities(config)?;
            let manifest = archive::read_manifest(archive, &keys)?;
            archive::verify_archive_anchor(
                &anchor_path(archive),
                &manifest,
                &keys,
                metadata_key(config)?.expose_secret().as_bytes(),
            )?;
            check_quota(
                &store,
                config.destinations[name].quota_bytes,
                &[
                    (object_key.clone(), archive.clone()),
                    (
                        format!("{object_key}.manifest"),
                        archive::manifest_path(archive),
                    ),
                    (format!("{object_key}.anchor"), anchor_path(archive)),
                ],
            )
            .with_context(|| format!("{name}: upload quota check"))?;
            let payload = store.put_immutable(&object_key, archive)?;
            ensure!(payload.verified, "archive upload not verified");
            let anchor =
                store.put_immutable(&format!("{object_key}.anchor"), &anchor_path(archive))?;
            ensure!(
                anchor.verified,
                "archive authentication anchor not verified"
            );
            let metadata = store.put_immutable(
                &format!("{object_key}.manifest"),
                &archive::manifest_path(archive),
            )?;
            ensure!(metadata.verified, "archive manifest not verified");
            state.cache_manifest(name, &run.id, &manifest)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                receipt.state = ReplicaState::Verified;
                receipt.verified_at = Some(Utc::now());
                receipt.full_verified_at = receipt.verified_at;
            }
            Err(error) => {
                receipt.state = ReplicaState::Failed;
                receipt.error = Some(format!("{error:#}"));
            }
        }
        run.replicas.insert(name.clone(), receipt);
        state.save_run(run)?;
    }
    let complete = plan.targets.iter().all(|name| {
        run.replicas
            .get(name)
            .is_some_and(|r| r.state == ReplicaState::Verified)
    });
    run.state = if complete {
        RunState::Committed
    } else {
        RunState::Failed
    };
    run.error = (!complete).then(|| "one or more uploads failed; use dcloud retry".into());
    state.save_run(run)?;
    if !complete {
        bail!(
            "upload {} incomplete: {}",
            run.id,
            serde_json::to_string(&run.replicas)?
        );
    }
    let mut cleanup_pending = None;
    if let Some(cleanup) = &plan.cleanup
        && state
            .load_value::<String>("upload-cleanup", &run.id)?
            .is_none()
    {
        let outcome = (|| -> Result<()> {
            let _expiry_lock = state.lock("archive-expiration")?;
            for name in &plan.targets {
                if !run.replicas[name]
                    .full_verified_at
                    .is_some_and(|verified| verified >= Utc::now() - chrono::Duration::minutes(15))
                {
                    verify_replica(config, run, name)?;
                }
            }
            state.save_run(run)?;
            let evidence = CleanupEvidence::from_run(run, plan.fingerprint.clone())?;
            let ticket = policy::quarantine(
                state,
                &plan.source,
                cleanup,
                evidence,
                Utc::now(),
                fingerprint,
            )?;
            state.save_value("cleanup-policy", &ticket.id, cleanup)?;
            state.save_value("upload-cleanup", &run.id, &ticket.id)?;
            Ok(())
        })();
        if let Err(error) = outcome {
            cleanup_pending = Some(format!("{error:#}"));
        }
    }
    run.error = cleanup_pending.clone();
    state.save_run(run)?;
    if cleanup_pending.is_none() {
        let stage = config.state_dir.join("uploads").join(&run.id);
        if stage.try_exists()? {
            fs::remove_dir_all(stage)?;
        }
    }
    Ok(
        json!({"id":run.id,"state":run.state,"replicas":run.replicas,"cleanup_pending":cleanup_pending}),
    )
}

fn validate_plan(config: &Config, run: &RunRecord, plan: &UploadPlan) -> Result<()> {
    ensure!(
        plan.options.id.as_deref() == Some(run.id.as_str())
            && plan.options.host == run.host
            && plan.options.job == "_uploads",
        "upload plan identity mismatch"
    );
    let targets: std::collections::BTreeSet<_> = plan.targets.iter().collect();
    ensure!(
        !targets.is_empty()
            && targets.len() == plan.targets.len()
            && targets == plan.variants.keys().collect(),
        "upload plan destination mismatch"
    );
    for (name, path) in &plan.variants {
        let destination = config
            .destinations
            .get(name)
            .context("upload destination is no longer configured")?;
        let expected =
            config
                .state_dir
                .join("uploads")
                .join(&run.id)
                .join(if destination.encrypted {
                    "encrypted.dcloud"
                } else {
                    "plain.dcloud"
                });
        ensure!(path == &expected, "upload plan spool path mismatch");
    }
    Ok(())
}

pub fn verify_replica(config: &Config, run: &mut RunRecord, name: &str) -> Result<()> {
    let stage = tempfile::tempdir_in(&config.state_dir)?;
    let archive = stage.path().join("verify.dcloud");
    fetch_archive(config, &run.id, &run.host, name, &archive)?;
    let keys = identities(config)?;
    archive::verify_with_limits(&archive, &keys, restore_limits(config))?;
    let receipt = run
        .replicas
        .get_mut(name)
        .context("archive replica is absent from the run")?;
    receipt.state = ReplicaState::Verified;
    receipt.verified_at = Some(Utc::now());
    receipt.full_verified_at = receipt.verified_at;
    receipt.error = None;
    Ok(())
}

pub fn retry(config: &Config, state: &mut State, id: Option<&str>) -> Result<Value> {
    let mut results = Vec::new();
    for mut run in state
        .runs()?
        .into_iter()
        .filter(|r| r.job == "_uploads" && r.host == config.host && id.is_none_or(|id| id == r.id))
    {
        let _lock = state.lock(&format!("upload:{}", run.id))?;
        let mut plan: UploadPlan = state
            .load_value("upload-plan", &run.id)?
            .context("upload planning incomplete; start a new upload")?;
        if run.state == RunState::Committed
            && (plan.cleanup.is_none()
                || state
                    .load_value::<String>("upload-cleanup", &run.id)?
                    .is_some())
        {
            continue;
        }
        let _source_lock = state.lock(&format!("upload-source:{}", plan.source.display()))?;
        prepare(config, state, &mut run, &mut plan)?;
        results.push(finish(config, state, &mut run, &plan)?);
    }
    Ok(json!(results))
}

pub fn download(
    config: &Config,
    id: &str,
    host: &str,
    from: &str,
    destination: &Path,
    selection: &[PathBuf],
) -> Result<Manifest> {
    ensure!(
        fs::symlink_metadata(destination).is_err(),
        "restore destination already exists"
    );
    let stage = tempfile::tempdir_in(&config.state_dir)?;
    let archive = stage.path().join("download.dcloud");
    fetch_archive(config, id, host, from, &archive)?;
    let identities = identities(config)?;
    archive::restore_with_limits(
        &archive,
        destination,
        &identities,
        selection,
        restore_limits(config),
    )
}

fn fetch_archive(
    config: &Config,
    id: &str,
    host: &str,
    from: &str,
    archive: &Path,
) -> Result<Manifest> {
    let object_key = key(host, id)?;
    let source_store = store(config, from)?;
    let sidecar = source_store.get_limited(
        &format!("{object_key}.manifest"),
        &archive::manifest_path(archive),
        64 * 1024 * 1024,
    );
    if let Err(error) = sidecar {
        let present = source_store
            .list_uploads(Some(host))?
            .iter()
            .any(|object| object.key == format!("{object_key}.manifest"));
        if present {
            return Err(error.context("archive sidecar could not be downloaded"));
        }
        source_store.get_limited(&object_key, archive, config.uploads.max_download_bytes)?;
    }
    let identities = identities(config)?;
    let mut manifest = archive::read_manifest(archive, &identities)?;
    ensure!(
        manifest.id == id && manifest.host == host,
        "downloaded archive identity mismatch"
    );
    ensure!(
        !config.destinations[from].encrypted || manifest.encrypted,
        "encrypted destination contains an unencrypted archive"
    );
    ensure!(
        manifest.total_bytes <= config.uploads.max_restore_bytes,
        "archive exceeds configured restore size limit"
    );
    if manifest.archive_sha256.is_none() {
        ensure!(
            !archive::manifest_path(archive).try_exists()?,
            "published archive manifest lacks its payload checksum"
        );
        manifest.archive_sha256 = Some(archive::hash_file(archive)?);
    }
    fetch_anchor(
        config,
        &source_store,
        &object_key,
        &manifest,
        &identities,
        &anchor_path(archive),
    )?;
    if !archive.try_exists()? {
        source_store.get_limited(&object_key, archive, config.uploads.max_download_bytes)?;
    }
    Ok(manifest)
}

fn fetch_anchor(
    config: &Config,
    store: &Store,
    object_key: &str,
    manifest: &Manifest,
    identities: &[String],
    path: &Path,
) -> Result<()> {
    store.get_limited(&format!("{object_key}.anchor"), path, archive::MAX_METADATA_BYTES)
        .context("archive authentication anchor is missing or unavailable; unsigned remote archives cannot be trusted")?;
    archive::verify_archive_anchor(
        path,
        manifest,
        identities,
        metadata_key(config)?.expose_secret().as_bytes(),
    )
}

fn restore_limits(config: &Config) -> archive::RestoreLimits {
    archive::RestoreLimits {
        max_total_bytes: config.uploads.max_restore_bytes,
        max_file_bytes: config.uploads.max_restore_bytes,
    }
}

pub fn manifests(config: &Config, from: &str, host: Option<&str>) -> Result<Vec<Manifest>> {
    let store = store(config, from)?;
    let identities = identities(config)?;
    let stage = tempfile::tempdir_in(&config.state_dir)?;
    let mut result = Vec::new();
    let items = store.list_uploads(host)?;
    for item in &items {
        if !item.key.ends_with(".dcloud.manifest") || !item.key.contains("/uploads/") {
            continue;
        }
        ensure!(
            item.bytes <= 64 * 1024 * 1024,
            "manifest size limit exceeded"
        );
        let path = stage.path().join("manifest");
        store.get_limited(&item.key, &path, 64 * 1024 * 1024)?;
        let mut manifest = archive::read_sidecar(&path, &identities)?;
        ensure!(
            !config.destinations[from].encrypted || manifest.encrypted,
            "encrypted destination contains an unencrypted manifest"
        );
        ensure!(
            item.key == format!("{}.manifest", key(&manifest.host, &manifest.id)?),
            "archive manifest identity mismatch"
        );
        fetch_anchor(
            config,
            &store,
            &key(&manifest.host, &manifest.id)?,
            &manifest,
            &identities,
            &stage.path().join("anchor"),
        )?;
        let changes =
            metadata_changes(config, &store, &items, &manifest, &identities, stage.path())?;
        apply_metadata(&mut manifest, &changes)?;
        result.push(manifest);
    }
    Ok(result)
}

fn metadata_key(config: &Config) -> Result<SecretString> {
    use std::io::Read;
    crate::setup::check_secret(&config.password_file)
        .context("archive labels require the repository recovery password")?;
    let mut password = String::new();
    fs::File::open(&config.password_file)?
        .take(65537)
        .read_to_string(&mut password)?;
    ensure!(
        password.len() <= 65536,
        "repository recovery password exceeds size limit"
    );
    let password = password.trim_end_matches(['\n', '\r']);
    ensure!(
        password.len() >= 32 && !password.contains(['\n', '\r', '\0']),
        "invalid repository recovery password for metadata authentication"
    );
    Ok(SecretString::from(password.to_owned()))
}

fn metadata_object_key(host: &str, id: &str, revision: &str) -> Result<String> {
    uuid::Uuid::parse_str(revision).context("invalid metadata revision")?;
    Ok(format!("{}.metadata/{revision}.dcloud", key(host, id)?))
}

fn metadata_changes(
    config: &Config,
    store: &Store,
    items: &[ObjectInfo],
    manifest: &Manifest,
    identities: &[String],
    stage: &Path,
) -> Result<Vec<MetadataChange>> {
    let prefix = format!("{}.metadata/", key(&manifest.host, &manifest.id)?);
    let records: Vec<_> = items
        .iter()
        .filter(|item| item.key.starts_with(&prefix))
        .collect();
    ensure!(
        records.len() <= MAX_METADATA_REVISIONS,
        "archive metadata history limit exceeded"
    );
    let total = records.iter().try_fold(0u64, |total, item| {
        total
            .checked_add(item.bytes)
            .context("metadata size overflow")
    })?;
    ensure!(
        total <= MAX_METADATA_HISTORY_BYTES,
        "archive metadata history size limit exceeded"
    );
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let authentication_key = metadata_key(config)?;
    let expected_hash = manifest
        .archive_sha256
        .as_ref()
        .context("archive metadata requires the original payload checksum")?;
    let mut result = Vec::new();
    for item in records {
        ensure!(
            item.bytes <= archive::MAX_METADATA_BYTES,
            "metadata change exceeds size limit"
        );
        let path = stage.join("metadata.dcloud");
        store.get_limited(&item.key, &path, archive::MAX_METADATA_BYTES)?;
        let change = archive::read_metadata_change(
            &path,
            identities,
            authentication_key.expose_secret().as_bytes(),
        )?;
        ensure!(
            change.archive_id == manifest.id
                && change.host == manifest.host
                && &change.archive_sha256 == expected_hash,
            "archive metadata identity or payload checksum mismatch"
        );
        ensure!(
            change.encrypted == manifest.encrypted,
            "archive metadata encryption policy mismatch"
        );
        ensure!(
            item.key == metadata_object_key(&change.host, &change.archive_id, &change.revision)?,
            "archive metadata revision identity mismatch"
        );
        result.push(change);
    }
    result.sort_by(|left, right| {
        (left.sequence, &left.revision).cmp(&(right.sequence, &right.revision))
    });
    ensure!(
        result
            .windows(2)
            .all(|pair| pair[0].revision != pair[1].revision),
        "duplicate archive metadata revision"
    );
    Ok(result)
}

fn apply_metadata(manifest: &mut Manifest, changes: &[MetadataChange]) -> Result<()> {
    let mut labels: std::collections::BTreeSet<_> = manifest.labels.iter().cloned().collect();
    for change in changes {
        if let Some(category) = &change.category {
            manifest.category = category.clone();
        }
        for label in &change.remove {
            labels.remove(label);
        }
        labels.extend(change.add.iter().cloned());
        ensure!(labels.len() <= 1024, "archive label count limit exceeded");
    }
    manifest.labels = labels.into_iter().collect();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn relabel(
    config: &Config,
    state: &State,
    from: &str,
    host: &str,
    id: &str,
    category: Option<&str>,
    add: &[String],
    remove: &[String],
) -> Result<Value> {
    ensure!(
        category.is_some() || !add.is_empty() || !remove.is_empty(),
        "provide a category or label change"
    );
    let object_key = key(host, id)?;
    let _expiry_lock = state.lock("archive-expiration")?;
    let _lock = state.lock(&format!("archive-metadata:{from}:{host}:{id}"))?;
    let store = store(config, from)?;
    let identities = identities(config)?;
    let authentication_key = metadata_key(config)?;
    let stage = tempfile::tempdir_in(&config.state_dir)?;
    let sidecar = stage.path().join("manifest");
    store.get_limited(
        &format!("{object_key}.manifest"),
        &sidecar,
        64 * 1024 * 1024,
    )?;
    let mut manifest = archive::read_sidecar(&sidecar, &identities)?;
    ensure!(
        manifest.id == id && manifest.host == host,
        "archive manifest identity mismatch"
    );
    ensure!(
        !config.destinations[from].encrypted || manifest.encrypted,
        "encrypted destination contains an unencrypted manifest"
    );
    ensure!(
        !manifest.expires_at.is_some_and(|time| time <= Utc::now()),
        "expired archives cannot be relabeled"
    );
    fetch_anchor(
        config,
        &store,
        &object_key,
        &manifest,
        &identities,
        &stage.path().join("anchor"),
    )?;
    let items = store.list_uploads(Some(host))?;
    ensure!(
        items.iter().any(|item| item.key == object_key),
        "archive payload is missing"
    );
    let mut changes =
        metadata_changes(config, &store, &items, &manifest, &identities, stage.path())?;
    ensure!(
        changes.len() < MAX_METADATA_REVISIONS,
        "archive metadata history limit reached"
    );
    let change = MetadataChange {
        format_version: 1,
        archive_id: id.into(),
        host: host.into(),
        archive_sha256: manifest
            .archive_sha256
            .clone()
            .context("archive has no payload checksum")?,
        revision: uuid::Uuid::new_v4().to_string(),
        sequence: changes
            .iter()
            .map(|change| change.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("archive metadata sequence exhausted")?,
        created_at: Utc::now(),
        encrypted: manifest.encrypted,
        category: category.map(str::to_owned),
        add: add.to_vec(),
        remove: remove.to_vec(),
    };
    let recipients = if manifest.encrypted {
        &config.recipients[..]
    } else {
        &[]
    };
    let path = stage.path().join("new-metadata.dcloud");
    archive::write_metadata_change(
        &path,
        &change,
        recipients,
        authentication_key.expose_secret().as_bytes(),
    )?;
    changes.push(change.clone());
    apply_metadata(&mut manifest, &changes)?;
    let prefix = format!("{object_key}.metadata/");
    let history_bytes = items
        .iter()
        .filter(|item| item.key.starts_with(&prefix))
        .try_fold(0u64, |total, item| {
            total
                .checked_add(item.bytes)
                .context("metadata size overflow")
        })?;
    ensure!(
        history_bytes.saturating_add(fs::metadata(&path)?.len()) <= MAX_METADATA_HISTORY_BYTES,
        "archive metadata history size limit reached"
    );
    let change_key = metadata_object_key(host, id, &change.revision)?;
    check_quota(
        &store,
        config.destinations[from].quota_bytes,
        &[(change_key.clone(), path.clone())],
    )?;
    let receipt = store.put_immutable(&change_key, &path)?;
    ensure!(
        receipt.verified,
        "archive metadata publication was not verified"
    );
    state.cache_manifest(from, id, &manifest)?;
    Ok(
        json!({"id":id,"host":host,"destination":from,"revision":change.revision,"category":manifest.category,"labels":manifest.labels,"verified":true}),
    )
}

fn check_quota(store: &Store, quota: Option<u64>, incoming: &[(String, PathBuf)]) -> Result<()> {
    let Some(quota) = quota else {
        return Ok(());
    };
    let existing = store.list("")?;
    let used = existing.iter().try_fold(0u64, |total, object| {
        total
            .checked_add(object.bytes)
            .context("storage size overflow")
    })?;
    let added = incoming
        .iter()
        .filter(|(key, _)| !existing.iter().any(|object| object.key == *key))
        .try_fold(0u64, |total, (_, path)| {
            total
                .checked_add(fs::metadata(path)?.len())
                .context("upload size overflow")
        })?;
    ensure!(
        used.checked_add(added).is_some_and(|total| total <= quota),
        "configured storage quota would be exceeded"
    );
    Ok(())
}

pub fn expire(config: &Config, state: &State, apply: bool) -> Result<Value> {
    let mut results = Vec::new();
    let _lock = state.lock("archive-expiration")?;
    let protected: std::collections::BTreeSet<_> = state
        .list_values::<policy::QuarantineTicket>("quarantine")?
        .into_iter()
        .filter(|(_, ticket)| {
            matches!(
                ticket.state,
                policy::QuarantineState::Prepared
                    | policy::QuarantineState::Held
                    | policy::QuarantineState::Changed
            )
        })
        .map(|(_, ticket)| ticket.evidence.run_id)
        .collect();
    let mut intents: BTreeMap<_, ExpirationIntent> = state
        .list_values::<ExpirationIntent>("archive-expiration")?
        .into_iter()
        .filter(|(_, intent)| !intent.complete)
        .collect();
    for (name, destination) in &config.destinations {
        if destination.kind == DestinationKind::Rest {
            continue;
        }
        for manifest in manifests(config, name, Some(&config.host))? {
            if !manifest.expires_at.is_some_and(|time| time <= Utc::now()) {
                continue;
            }
            if protected.contains(&manifest.id) {
                results.push(json!({"destination":name,"id":manifest.id,"deleted":false,"protected":"source remains in quarantine"}));
                continue;
            }
            let intent_key = format!("{name}:{}:{}", manifest.host, manifest.id);
            intents.entry(intent_key).or_insert(ExpirationIntent {
                destination: name.clone(),
                host: manifest.host,
                id: manifest.id,
                expires_at: manifest.expires_at.context("archive has no expiry")?,
                config_hash: config.upload_digest(std::slice::from_ref(name))?,
                complete: false,
            });
        }
    }
    for (intent_key, mut intent) in intents {
        if protected.contains(&intent.id) {
            continue;
        }
        ensure!(
            intent.host == config.host
                && intent.config_hash
                    == config.upload_digest(std::slice::from_ref(&intent.destination))?,
            "archive expiry destination configuration changed; original destination is required to finish removal"
        );
        let object_key = key(&intent.host, &intent.id)?;
        if apply {
            state.save_value("archive-expiration", &intent_key, &intent)?;
            let store = store(config, &intent.destination)?;
            let prefix = format!("{object_key}.metadata/");
            for item in store
                .list_uploads(Some(&intent.host))?
                .into_iter()
                .filter(|item| item.key.starts_with(&prefix))
            {
                store.remove(&item.key)?;
            }
            store.remove(&format!("{object_key}.manifest"))?;
            store.remove(&object_key)?;
            store.remove(&format!("{object_key}.anchor"))?;
            intent.complete = true;
            state.save_value("archive-expiration", &intent_key, &intent)?;
        }
        results.push(json!({"destination":intent.destination,"id":intent.id,"expired":intent.expires_at,"deleted":apply}));
    }
    Ok(json!(results))
}

#[derive(Clone, Serialize, Deserialize)]
struct ExpirationIntent {
    destination: String,
    host: String,
    id: String,
    expires_at: chrono::DateTime<Utc>,
    config_hash: String,
    complete: bool,
}

#[cfg(test)]
#[path = "../tests/unit/objects_tests.rs"]
mod tests;
