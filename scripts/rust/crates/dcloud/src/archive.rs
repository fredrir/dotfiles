use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use filetime::FileTime;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::WalkDir;

const ARCHIVE_MAGIC: &[u8] = b"DCLOUD-ARCHIVE\0\x01";
const MANIFEST_MAGIC: &[u8] = b"DCLOUD-MANIFEST\0\x01";
const METADATA_MAGIC: &[u8] = b"DCLOUD-METADATA\0\x01";
const METADATA_AUTH_DOMAIN: &[u8] = b"dcloud/portable-archive/metadata/hmac-sha256/v1\0";
const ANCHOR_MAGIC: &[u8] = b"DCLOUD-ANCHOR\0\x01";
const ANCHOR_AUTH_DOMAIN: &[u8] = b"dcloud/portable-archive/anchor/hmac-sha256/v1\0";
pub const MAX_METADATA_BYTES: u64 = 64 * 1024;
const INNER_MAGIC: &[u8] = b"DCLOUD-AUTHENTICATED\0\x01";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_TRAILING_PADDING: u64 = 1024 * 1024;
const MAX_PATH_BYTES: u64 = 16 * 1024;
const MAX_AGE_HEADER_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveOptions {
    pub id: Option<String>,
    pub host: String,
    pub job: String,
    pub category: String,
    pub labels: Vec<String>,
    pub recipients: Vec<String>,
    pub compression_level: i32,
    pub compression_threads: u32,
    pub excludes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_output_bytes: u64,
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        Self {
            id: None,
            host: String::new(),
            job: String::new(),
            category: String::new(),
            labels: Vec::new(),
            recipients: Vec::new(),
            compression_level: 3,
            compression_threads: 1,
            excludes: Vec::new(),
            expires_at: None,
            max_output_bytes: u64::MAX,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryFingerprint {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub sha256: Option<String>,
    pub modified_seconds: i64,
    pub modified_nanos: u32,
    pub mode: u32,
    pub link_target: Option<String>,
    #[serde(default)]
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub host: String,
    pub job: String,
    pub category: String,
    pub labels: Vec<String>,
    pub source_name: String,
    pub source_is_dir: bool,
    pub encrypted: bool,
    pub compression: String,
    pub entries: Vec<EntryFingerprint>,
    pub total_bytes: u64,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub archive_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataChange {
    pub format_version: u32,
    pub archive_id: String,
    pub host: String,
    pub archive_sha256: String,
    pub revision: String,
    pub sequence: u64,
    pub created_at: DateTime<Utc>,
    pub encrypted: bool,
    pub category: Option<String>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedDocument<T> {
    document: T,
    hmac_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveAnchor {
    format_version: u32,
    archive_id: String,
    host: String,
    archive_sha256: String,
    manifest_sha256: String,
    encrypted: bool,
}

fn archive_anchor(manifest: &Manifest) -> Result<ArchiveAnchor> {
    validate_manifest(manifest)?;
    Ok(ArchiveAnchor {
        format_version: 1,
        archive_id: manifest.id.clone(),
        host: manifest.host.clone(),
        archive_sha256: manifest
            .archive_sha256
            .clone()
            .context("archive authentication requires its payload checksum")?,
        manifest_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(manifest)?)),
        encrypted: manifest.encrypted,
    })
}

pub fn write_archive_anchor(
    output: &Path,
    manifest: &Manifest,
    recipients: &[String],
    authentication_key: &[u8],
) -> Result<()> {
    ensure!(
        manifest.encrypted == !recipients.is_empty(),
        "archive anchor encryption policy mismatch"
    );
    write_authenticated_document(
        output,
        ANCHOR_MAGIC,
        ANCHOR_AUTH_DOMAIN,
        recipients,
        authentication_key,
        &archive_anchor(manifest)?,
    )
}

pub fn verify_archive_anchor(
    path: &Path,
    manifest: &Manifest,
    identities: &[String],
    authentication_key: &[u8],
) -> Result<()> {
    let (anchor, encrypted): (ArchiveAnchor, bool) = read_authenticated_document(
        path,
        ANCHOR_MAGIC,
        ANCHOR_AUTH_DOMAIN,
        identities,
        authentication_key,
    )?;
    ensure!(
        encrypted == manifest.encrypted && anchor == archive_anchor(manifest)?,
        "archive authentication anchor does not match its original manifest and payload"
    );
    Ok(())
}

pub fn write_metadata_change(
    output: &Path,
    change: &MetadataChange,
    recipients: &[String],
    authentication_key: &[u8],
) -> Result<()> {
    validate_metadata_change(change)?;
    ensure!(
        change.encrypted == !recipients.is_empty(),
        "metadata encryption policy mismatch"
    );
    write_authenticated_document(
        output,
        METADATA_MAGIC,
        METADATA_AUTH_DOMAIN,
        recipients,
        authentication_key,
        change,
    )
}

fn write_authenticated_document<T: Serialize>(
    output: &Path,
    magic: &[u8],
    domain: &[u8],
    recipients: &[String],
    authentication_key: &[u8],
    document: &T,
) -> Result<()> {
    let mut mac = metadata_mac(authentication_key, domain)?;
    mac.update(&serde_json::to_vec(document)?);
    let document = serde_json::to_vec(&SignedDocument {
        document,
        hmac_sha256: mac.finalize().into_bytes().into(),
    })?;
    ensure!(
        document.len() <= 48 * 1024,
        "metadata change exceeds size limit"
    );
    let mut file =
        NamedTempFile::new_in(output.parent().context("metadata output has no parent")?)?;
    write_sealed(
        &mut BoundedWriter {
            inner: file.as_file_mut(),
            remaining: MAX_METADATA_BYTES,
        },
        magic,
        recipients,
        |writer| write_document(writer, &document),
    )?;
    file.as_file().sync_all()?;
    file.persist_noclobber(output)?;
    sync_parent(output.parent().context("metadata output has no parent")?)?;
    Ok(())
}

pub fn read_metadata_change(
    path: &Path,
    identities: &[String],
    authentication_key: &[u8],
) -> Result<MetadataChange> {
    let (change, encrypted): (MetadataChange, bool) = read_authenticated_document(
        path,
        METADATA_MAGIC,
        METADATA_AUTH_DOMAIN,
        identities,
        authentication_key,
    )?;
    ensure!(
        change.encrypted == encrypted,
        "metadata encryption header mismatch"
    );
    validate_metadata_change(&change)?;
    Ok(change)
}

fn read_authenticated_document<T: Serialize + serde::de::DeserializeOwned>(
    path: &Path,
    magic: &[u8],
    domain: &[u8],
    identities: &[String],
    authentication_key: &[u8],
) -> Result<(T, bool)> {
    ensure!(
        open_regular(path)?.metadata()?.len() <= MAX_METADATA_BYTES,
        "metadata change exceeds size limit"
    );
    let (mut reader, encrypted) = open_sealed(path, magic, identities)?;
    let document = read_document_limited(&mut reader, MAX_METADATA_BYTES)?;
    let mut eof = [0u8; 1];
    ensure!(reader.read(&mut eof)? == 0, "trailing metadata content");
    let signed: SignedDocument<T> = serde_json::from_slice(&document)?;
    let mut mac = metadata_mac(authentication_key, domain)?;
    mac.update(&serde_json::to_vec(&signed.document)?);
    mac.verify_slice(&signed.hmac_sha256).map_err(|_| anyhow::anyhow!(
        "archive metadata authentication failed; restore the original repository recovery password if it was replaced"
    ))?;
    Ok((signed.document, encrypted))
}

fn metadata_mac(key: &[u8], domain: &[u8]) -> Result<Hmac<Sha256>> {
    ensure!(
        key.len() >= 32 && key.len() <= 64 * 1024,
        "metadata authentication requires the repository recovery password (32..65536 bytes)"
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(key)?;
    mac.update(domain);
    Ok(mac)
}

fn validate_metadata_change(change: &MetadataChange) -> Result<()> {
    ensure!(
        change.format_version == 1,
        "unsupported metadata format version"
    );
    Uuid::parse_str(&change.archive_id).context("invalid metadata archive ID")?;
    Uuid::parse_str(&change.revision).context("invalid metadata revision")?;
    ensure!(change.sequence > 0, "invalid metadata sequence");
    ensure!(
        change.host.len() <= 128
            && !change.host.is_empty()
            && !change.host.chars().any(char::is_control),
        "invalid metadata host"
    );
    ensure!(
        change.archive_sha256.len() == 64
            && change
                .archive_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid metadata archive hash"
    );
    ensure!(
        change
            .category
            .as_ref()
            .is_none_or(|value| value.len() <= 512 && !value.chars().any(char::is_control)),
        "invalid metadata category"
    );
    ensure!(
        change.add.len() + change.remove.len() <= 128,
        "metadata label change limit exceeded"
    );
    for label in change.add.iter().chain(&change.remove) {
        ensure!(
            !label.is_empty() && label.len() <= 256 && !label.chars().any(char::is_control),
            "invalid metadata label"
        );
    }
    ensure!(
        !change.add.iter().any(|label| change.remove.contains(label)),
        "cannot add and remove the same label"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct RestoreLimits {
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for RestoreLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 16 * 1024 * 1024 * 1024 * 1024,
            max_file_bytes: 8 * 1024 * 1024 * 1024 * 1024,
        }
    }
}

pub fn manifest_path(archive: &Path) -> PathBuf {
    let mut name = archive.as_os_str().to_os_string();
    name.push(".manifest");
    PathBuf::from(name)
}

pub fn generate_identity() -> (String, String) {
    let identity = age::x25519::Identity::generate();
    (
        identity.to_string().expose_secret().to_string(),
        identity.to_public().to_string(),
    )
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut input = open_regular(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut input, &mut HashSink(&mut hasher))?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn tree_digest(source: &Path) -> Result<String> {
    let mut entries = hash_tree(source, &[])?;
    if !source.is_dir() {
        for entry in &mut entries {
            entry.path = ".".to_owned();
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&entries)?)
    ))
}

pub fn hash_tree(source: &Path, excludes: &[String]) -> Result<Vec<EntryFingerprint>> {
    hash_tree_internal(source, excludes, true)
}

fn hash_tree_internal(
    source: &Path,
    excludes: &[String],
    hash_contents: bool,
) -> Result<Vec<EntryFingerprint>> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("cannot inspect source {}", source.display()))?;
    ensure!(
        metadata.is_dir() || metadata.is_file(),
        "source must be a regular file or directory"
    );
    let exclusions = exclusions(excludes)?;
    let mut result = Vec::new();
    if metadata.is_file() {
        let name = source.file_name().context("source file has no name")?;
        let path = name.to_str().context("source file name is not UTF-8")?;
        ensure!(!exclusions.is_match(path), "source file is excluded");
        result.push(fingerprint(source, path, hash_contents)?);
        return Ok(result);
    }
    for entry in WalkDir::new(source)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !excluded(
                    entry.path().strip_prefix(source).unwrap_or(entry.path()),
                    &exclusions,
                )
        })
    {
        let entry = entry.with_context(|| format!("cannot enumerate {}", source.display()))?;
        let relative = entry.path().strip_prefix(source)?;
        let path = if relative.as_os_str().is_empty() {
            "."
        } else {
            relative.to_str().context("source path is not UTF-8")?
        };
        result.push(fingerprint(entry.path(), path, hash_contents)?);
        ensure!(result.len() <= MAX_ENTRIES, "archive entry limit exceeded");
    }
    Ok(result)
}

pub fn create(source: &Path, output: &Path, options: &ArchiveOptions) -> Result<Manifest> {
    ensure!(
        (1..=22).contains(&options.compression_level),
        "compression level must be between 1 and 22"
    );
    ensure!(
        options.compression_threads <= 64,
        "compression thread limit is 64"
    );
    ensure!(
        fs::symlink_metadata(output).is_err(),
        "archive already exists"
    );
    let sidecar_path = manifest_path(output);
    ensure!(
        fs::symlink_metadata(&sidecar_path).is_err(),
        "manifest already exists"
    );
    let parent = existing_parent(output)?;
    let original_metadata = fs::symlink_metadata(source)?;
    ensure!(
        original_metadata.is_dir() || original_metadata.is_file(),
        "source must be a regular file or directory"
    );
    let source = source.canonicalize().context("cannot resolve source")?;
    ensure!(
        !parent.starts_with(&source),
        "archive output cannot be inside the source"
    );
    let entries = hash_tree(&source, &options.excludes)?;
    let total_bytes = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.size)
            .context("archive byte count overflow")
    })?;
    let mut manifest = Manifest {
        format_version: 1,
        id: options
            .id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        created_at: Utc::now(),
        host: options.host.clone(),
        job: options.job.clone(),
        category: options.category.clone(),
        labels: options.labels.clone(),
        source_name: source
            .file_name()
            .context("filesystem root cannot be archived")?
            .to_str()
            .context("source name is not UTF-8")?
            .to_owned(),
        source_is_dir: source.is_dir(),
        encrypted: !options.recipients.is_empty(),
        compression: "zstd".to_owned(),
        entries,
        total_bytes,
        expires_at: options.expires_at,
        archive_sha256: None,
    };
    validate_manifest(&manifest)?;
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    ensure!(
        manifest_bytes.len() as u64 <= MAX_MANIFEST_BYTES,
        "manifest is too large"
    );
    let mut archive_file = NamedTempFile::new_in(&parent)?;
    let sidecar_reserve = (manifest_bytes.len() as u64)
        .saturating_add(1024)
        .saturating_add((manifest_bytes.len() as u64 / 65536 + 1) * 16)
        .saturating_add(options.recipients.len() as u64 * 512);
    let archive_limit = options
        .max_output_bytes
        .checked_sub(sidecar_reserve)
        .context("archive manifest exceeds the available spool budget")?;
    write_sealed(
        &mut BoundedWriter {
            inner: archive_file.as_file_mut(),
            remaining: archive_limit,
        },
        ARCHIVE_MAGIC,
        &options.recipients,
        |writer| {
            write_document(writer, &manifest_bytes)?;
            let mut compressor =
                zstd::stream::write::Encoder::new(writer, options.compression_level)?;
            compressor.multithread(options.compression_threads)?;
            compressor.include_checksum(true)?;
            let mut builder = tar::Builder::new(compressor);
            builder.follow_symlinks(false);
            for entry in &manifest.entries {
                append_entry(&mut builder, &source, manifest.source_is_dir, entry)?;
            }
            builder.finish()?;
            let compressor = builder.into_inner()?;
            compressor.finish()?;
            Ok(())
        },
    )?;
    archive_file.as_file().sync_all()?;
    let after = hash_tree_internal(&source, &options.excludes, false)?;
    ensure!(
        after.len() == manifest.entries.len()
            && after.iter().zip(&manifest.entries).all(|(after, before)| {
                let mut before = before.clone();
                before.sha256 = None;
                after == &before
            }),
        "source changed while creating the archive; retry after it becomes stable"
    );
    manifest.archive_sha256 = Some(hash_file(archive_file.path())?);
    let sidecar_bytes = serde_json::to_vec(&manifest)?;
    let mut sidecar = NamedTempFile::new_in(&parent)?;
    let sidecar_limit = options
        .max_output_bytes
        .checked_sub(archive_file.as_file().metadata()?.len())
        .context("archive exceeds the spool budget")?;
    write_sealed(
        &mut BoundedWriter {
            inner: sidecar.as_file_mut(),
            remaining: sidecar_limit,
        },
        MANIFEST_MAGIC,
        &options.recipients,
        |writer| write_document(writer, &sidecar_bytes),
    )?;
    sidecar.as_file().sync_all()?;
    archive_file
        .persist_noclobber(output)
        .map_err(|error| error.error)?;
    if let Err(error) = sidecar.persist_noclobber(&sidecar_path) {
        let _ = fs::remove_file(output);
        return Err(error.error.into());
    }
    sync_parent(&parent)?;
    Ok(manifest)
}

pub fn read_manifest(archive: &Path, identities: &[String]) -> Result<Manifest> {
    if manifest_path(archive).try_exists()? {
        read_sidecar(&manifest_path(archive), identities)
    } else {
        read_embedded_manifest(archive, identities)
    }
}

pub fn read_embedded_manifest(archive: &Path, identities: &[String]) -> Result<Manifest> {
    let (mut reader, encrypted) = open_sealed(archive, ARCHIVE_MAGIC, identities)?;
    let manifest: Manifest = serde_json::from_slice(&read_document(&mut reader)?)?;
    validate_manifest(&manifest)?;
    ensure!(
        manifest.encrypted == encrypted,
        "archive encryption marker mismatch"
    );
    Ok(manifest)
}

pub fn read_sidecar(sidecar: &Path, identities: &[String]) -> Result<Manifest> {
    let (mut reader, encrypted) = open_sealed(sidecar, MANIFEST_MAGIC, identities)?;
    let manifest: Manifest = serde_json::from_slice(&read_document(&mut reader)?)?;
    ensure_eof(&mut reader)?;
    validate_manifest(&manifest)?;
    ensure!(
        manifest.encrypted == encrypted,
        "manifest encryption marker mismatch"
    );
    Ok(manifest)
}

pub fn verify(archive: &Path, identities: &[String]) -> Result<Manifest> {
    verify_with_limits(archive, identities, RestoreLimits::default())
}

pub fn verify_with_limits(
    archive: &Path,
    identities: &[String],
    limits: RestoreLimits,
) -> Result<Manifest> {
    inspect(archive, identities, None, &[], limits)
}

pub fn restore(
    archive: &Path,
    destination: &Path,
    identities: &[String],
    selection: &[PathBuf],
) -> Result<Manifest> {
    restore_with_limits(
        archive,
        destination,
        identities,
        selection,
        RestoreLimits::default(),
    )
}

pub fn restore_with_limits(
    archive: &Path,
    destination: &Path,
    identities: &[String],
    selection: &[PathBuf],
    limits: RestoreLimits,
) -> Result<Manifest> {
    ensure!(
        fs::symlink_metadata(destination).is_err(),
        "restore destination already exists"
    );
    let parent = existing_parent(destination)?;
    let staging = tempfile::Builder::new()
        .prefix(".dcloud-restore-")
        .tempdir_in(&parent)?;
    let manifest = inspect(archive, identities, Some(staging.path()), selection, limits)?;
    ensure!(
        fs::symlink_metadata(destination).is_err(),
        "restore destination appeared during restore"
    );
    fs::rename(staging.path(), destination).context("cannot publish verified restore")?;
    sync_parent(&parent)?;
    Ok(manifest)
}

fn inspect(
    archive: &Path,
    identities: &[String],
    destination: Option<&Path>,
    selection: &[PathBuf],
    limits: RestoreLimits,
) -> Result<Manifest> {
    for path in selection {
        validate_relative(path, true)?;
    }
    let sidecar = read_manifest(archive, identities)?;
    ensure!(
        sidecar.total_bytes <= limits.max_total_bytes,
        "archive exceeds the configured decompressed size limit"
    );
    ensure!(
        sidecar
            .entries
            .iter()
            .all(|entry| entry.size <= limits.max_file_bytes),
        "archive exceeds the configured per-file size limit"
    );
    if let Some(expected) = &sidecar.archive_sha256 {
        ensure!(
            hash_file(archive)? == *expected,
            "archive ciphertext checksum mismatch"
        );
    }
    let (mut reader, encrypted) = open_sealed(archive, ARCHIVE_MAGIC, identities)?;
    let manifest: Manifest = serde_json::from_slice(&read_document(&mut reader)?)?;
    validate_manifest(&manifest)?;
    ensure!(
        manifest.encrypted == encrypted,
        "archive encryption marker mismatch"
    );
    let mut comparable_sidecar = sidecar.clone();
    comparable_sidecar.archive_sha256 = None;
    ensure!(
        manifest == comparable_sidecar,
        "archive and sidecar manifests differ"
    );
    for selected in selection {
        ensure!(
            manifest
                .entries
                .iter()
                .any(|entry| selected_entry(&entry.path, std::slice::from_ref(selected))),
            "selected path is absent from the archive: {}",
            selected.display()
        );
    }
    let expected: BTreeMap<&str, &EntryFingerprint> = manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut seen = BTreeSet::new();
    let mut decoder = zstd::stream::read::Decoder::new(reader)?;
    decoder.window_log_max(27)?;
    let mut tar = tar::Archive::new(decoder);
    let mut delayed = Vec::new();
    let mut long_name = None;
    let mut long_link = None;
    for result in tar.entries()?.raw(true) {
        let mut entry = result.context("invalid archive entry")?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() || kind.is_gnu_longlink() {
            ensure!(
                (1..=MAX_PATH_BYTES + 1).contains(&entry.size()),
                "archive path extension exceeds its size limit"
            );
            let slot = if kind.is_gnu_longname() {
                &mut long_name
            } else {
                &mut long_link
            };
            ensure!(slot.is_none(), "duplicate archive path extension");
            let mut path = Vec::new();
            entry.read_to_end(&mut path)?;
            ensure!(
                path.pop() == Some(0) && !path.contains(&0),
                "invalid archive path extension"
            );
            *slot = Some(PathBuf::from(
                String::from_utf8(path).context("archive path extension is not UTF-8")?,
            ));
            continue;
        }
        ensure!(
            kind.is_file() || kind.is_dir() || kind.is_symlink(),
            "unsupported archive entry type"
        );
        let entry_path = match long_name.take() {
            Some(path) => path,
            None => entry.path()?.into_owned(),
        };
        let extended_link = long_link.take();
        ensure!(
            extended_link.is_none() || kind.is_symlink(),
            "link extension on a non-symlink entry"
        );
        validate_relative(&entry_path, true)?;
        let path = normalized_path(&entry_path)?;
        ensure!(seen.insert(path.clone()), "duplicate archive entry: {path}");
        let fingerprint = expected
            .get(path.as_str())
            .context("archive contains an unlisted entry")?;
        let extract = destination.is_some() && selected_entry(&path, selection);
        let target = destination.map(|root| root.join(&entry_path));
        match fingerprint.kind {
            EntryKind::File => {
                ensure!(kind.is_file(), "archive entry type differs from manifest");
                ensure!(
                    entry.size() == fingerprint.size,
                    "archive file size differs from manifest"
                );
                let mut hasher = Sha256::new();
                let bytes = if extract {
                    let target = target.as_ref().unwrap();
                    fs::create_dir_all(target.parent().context("restored file has no parent")?)?;
                    let mut output = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(target)?;
                    let bytes = copy_hash(&mut entry, &mut output, &mut hasher)?;
                    output.sync_all()?;
                    delayed.push((target.clone(), *fingerprint));
                    bytes
                } else {
                    copy_hash(&mut entry, &mut io::sink(), &mut hasher)?
                };
                ensure!(bytes == fingerprint.size, "archive file is truncated");
                let digest = format!("{:x}", hasher.finalize());
                ensure!(
                    fingerprint.sha256.as_deref() == Some(digest.as_str()),
                    "archive file checksum mismatch: {path}"
                );
            }
            EntryKind::Directory => {
                ensure!(
                    kind.is_dir() && entry.size() == 0,
                    "invalid archive directory"
                );
                if extract {
                    let target = target.unwrap();
                    fs::create_dir_all(&target)?;
                    delayed.push((target, *fingerprint));
                }
            }
            EntryKind::Symlink => {
                ensure!(
                    kind.is_symlink() && entry.size() == 0,
                    "invalid archive symlink"
                );
                let link = match extended_link {
                    Some(link) => link,
                    None => entry
                        .link_name()?
                        .context("archive symlink has no target")?
                        .into_owned(),
                };
                ensure!(
                    link.to_str() == fingerprint.link_target.as_deref(),
                    "archive symlink target mismatch"
                );
                validate_link(&entry_path, &link)?;
                if extract {
                    delayed.push((target.unwrap(), *fingerprint));
                }
            }
        }
    }
    ensure!(
        long_name.is_none() && long_link.is_none(),
        "archive ends with an unused path extension"
    );
    ensure!(
        seen.len() == manifest.entries.len(),
        "archive is missing manifest entries"
    );
    let mut decoder = tar.into_inner();
    let mut padding = Vec::new();
    decoder
        .by_ref()
        .take(MAX_TRAILING_PADDING + 1)
        .read_to_end(&mut padding)?;
    ensure!(
        padding.len() as u64 <= MAX_TRAILING_PADDING,
        "archive has excessive trailing data"
    );
    ensure!(
        padding.iter().all(|byte| *byte == 0),
        "archive has unexpected trailing data"
    );
    ensure_eof(&mut decoder)?;
    let mut reader = decoder.finish();
    ensure_eof(&mut reader)?;
    if destination.is_some() {
        for (target, entry) in &delayed {
            if entry.kind == EntryKind::Symlink {
                fs::create_dir_all(target.parent().context("symlink has no parent")?)?;
                create_symlink(entry.link_target.as_ref().unwrap(), target)?;
            }
        }
        delayed.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
        for (target, entry) in delayed {
            restore_metadata(&target, entry)?;
        }
    }
    Ok(sidecar)
}

fn fingerprint(path: &Path, relative: &str, hash_contents: bool) -> Result<EntryFingerprint> {
    validate_relative(Path::new(relative), true)?;
    let metadata = fs::symlink_metadata(path)?;
    let modified = FileTime::from_last_modification_time(&metadata);
    let (kind, size, sha256, link_target) = if metadata.is_file() {
        (
            EntryKind::File,
            metadata.len(),
            if hash_contents {
                Some(hash_file(path)?)
            } else {
                None
            },
            None,
        )
    } else if metadata.is_dir() {
        (EntryKind::Directory, 0, None, None)
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        validate_link(Path::new(relative), &target)?;
        (
            EntryKind::Symlink,
            0,
            None,
            Some(
                target
                    .to_str()
                    .context("symlink target is not UTF-8")?
                    .to_owned(),
            ),
        )
    } else {
        bail!("unsupported source file type: {}", path.display());
    };
    let xattrs = read_xattrs(path)?;
    let after = fs::symlink_metadata(path)?;
    ensure!(
        same_metadata(&metadata, &after),
        "source changed while hashing: {}",
        path.display()
    );
    Ok(EntryFingerprint {
        path: relative.to_owned(),
        kind,
        size,
        sha256,
        modified_seconds: modified.unix_seconds(),
        modified_nanos: modified.nanoseconds(),
        mode: mode(&metadata),
        link_target,
        xattrs,
    })
}

fn append_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    source_is_dir: bool,
    entry: &EntryFingerprint,
) -> Result<()> {
    let path = if source_is_dir {
        source.join(&entry.path)
    } else {
        source.to_path_buf()
    };
    let mut header = tar::Header::new_gnu();
    header.set_mode(entry.mode);
    header.set_mtime(entry.modified_seconds.max(0) as u64);
    header.set_uid(0);
    header.set_gid(0);
    match entry.kind {
        EntryKind::File => {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(entry.size);
            header.set_cksum();
            let mut file = open_regular(&path)?;
            let before = file.metadata()?;
            let mut hasher = Sha256::new();
            let mut reader = HashReader {
                input: &mut file,
                hasher: &mut hasher,
            };
            builder.append_data(&mut header, &entry.path, &mut reader)?;
            let digest = format!("{:x}", hasher.finalize());
            ensure!(
                entry.sha256.as_deref() == Some(digest.as_str()),
                "source changed while archiving: {}",
                path.display()
            );
            ensure!(
                same_metadata(&before, &file.metadata()?),
                "source changed while archiving"
            );
        }
        EntryKind::Directory => {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, &entry.path, io::empty())?;
        }
        EntryKind::Symlink => {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_cksum();
            builder.append_link(
                &mut header,
                &entry.path,
                entry.link_target.as_ref().unwrap(),
            )?;
        }
    }
    Ok(())
}

fn write_sealed<F>(
    output: &mut dyn Write,
    magic: &[u8],
    recipients: &[String],
    write: F,
) -> Result<()>
where
    F: FnOnce(&mut dyn Write) -> Result<()>,
{
    output.write_all(magic)?;
    let encrypted = u8::from(!recipients.is_empty());
    output.write_all(&[encrypted])?;
    if encrypted == 1 {
        let recipients = recipients
            .iter()
            .map(|recipient| {
                recipient
                    .parse::<age::x25519::Recipient>()
                    .map_err(|_| anyhow::anyhow!("invalid age X25519 recipient"))
            })
            .collect::<Result<Vec<_>>>()?;
        let encryptor = age::Encryptor::with_recipients(
            recipients
                .iter()
                .map(|recipient| recipient as &dyn age::Recipient),
        )?;
        let mut encrypted_output = encryptor.wrap_output(output)?;
        encrypted_output.write_all(INNER_MAGIC)?;
        encrypted_output.write_all(&[encrypted])?;
        write(&mut encrypted_output)?;
        encrypted_output.finish()?;
    } else {
        output.write_all(INNER_MAGIC)?;
        output.write_all(&[encrypted])?;
        write(output)?;
        output.flush()?;
    }
    Ok(())
}

fn open_sealed(path: &Path, magic: &[u8], identities: &[String]) -> Result<(Box<dyn Read>, bool)> {
    let mut file = BufReader::new(open_regular(path)?);
    expect_bytes(&mut file, magic)?;
    let encrypted = read_byte(&mut file)?;
    let mut reader: Box<dyn Read> = match encrypted {
        0 => Box::new(file),
        1 => {
            ensure!(
                !identities.is_empty(),
                "an age identity is required for this archive"
            );
            let identities = identities
                .iter()
                .map(|identity| {
                    identity
                        .trim()
                        .parse::<age::x25519::Identity>()
                        .map_err(|_| anyhow::anyhow!("invalid age X25519 identity"))
                })
                .collect::<Result<Vec<_>>>()?;
            let header_budget = Rc::new(Cell::new(Some(MAX_AGE_HEADER_BYTES)));
            let decryptor = age::Decryptor::new_buffered(HeaderBudget {
                inner: file,
                remaining: header_budget.clone(),
            })?;
            header_budget.set(None);
            Box::new(
                decryptor.decrypt(
                    identities
                        .iter()
                        .map(|identity| identity as &dyn age::Identity),
                )?,
            )
        }
        _ => bail!("unsupported archive encryption mode"),
    };
    expect_bytes(&mut reader, INNER_MAGIC)?;
    ensure!(
        read_byte(&mut reader)? == encrypted,
        "archive security header mismatch"
    );
    Ok((reader, encrypted == 1))
}

fn write_document(writer: &mut dyn Write, document: &[u8]) -> Result<()> {
    ensure!(
        document.len() as u64 <= MAX_MANIFEST_BYTES,
        "manifest is too large"
    );
    writer.write_all(&(document.len() as u64).to_le_bytes())?;
    writer.write_all(document)?;
    Ok(())
}

fn read_document(reader: &mut dyn Read) -> Result<Vec<u8>> {
    read_document_limited(reader, MAX_MANIFEST_BYTES)
}

fn read_document_limited(reader: &mut dyn Read, maximum: u64) -> Result<Vec<u8>> {
    let mut length = [0u8; 8];
    reader.read_exact(&mut length)?;
    let length = u64::from_le_bytes(length);
    ensure!(length <= maximum, "manifest exceeds the size limit");
    let mut document = vec![0; length as usize];
    reader.read_exact(&mut document)?;
    Ok(document)
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.format_version == 1,
        "unsupported archive format version"
    );
    ensure!(
        manifest.compression == "zstd",
        "unsupported archive compression"
    );
    Uuid::parse_str(&manifest.id).context("invalid archive ID")?;
    ensure!(
        !manifest.entries.is_empty() && manifest.entries.len() <= MAX_ENTRIES,
        "invalid archive entry count"
    );
    let mut seen = BTreeSet::new();
    let mut total_bytes = 0u64;
    let entries: BTreeMap<_, _> = manifest
        .entries
        .iter()
        .map(|entry| (Path::new(&entry.path), entry))
        .collect();
    for entry in &manifest.entries {
        let path = Path::new(&entry.path);
        validate_relative(path, true)?;
        ensure!(
            normalized_path(path)? == entry.path,
            "noncanonical manifest path"
        );
        ensure!(seen.insert(entry.path.as_str()), "duplicate manifest path");
        ensure!(entry.modified_nanos < 1_000_000_000, "invalid timestamp");
        ensure!(entry.mode & !0o7777 == 0, "invalid file mode");
        match entry.kind {
            EntryKind::File => {
                let digest = entry
                    .sha256
                    .as_deref()
                    .context("file is missing its checksum")?;
                ensure!(
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "invalid file checksum"
                );
                ensure!(
                    entry.link_target.is_none(),
                    "regular file has a symlink target"
                );
            }
            EntryKind::Directory => ensure!(
                entry.size == 0 && entry.sha256.is_none() && entry.link_target.is_none(),
                "invalid directory metadata"
            ),
            EntryKind::Symlink => {
                ensure!(
                    entry.size == 0 && entry.sha256.is_none(),
                    "invalid symlink metadata"
                );
                validate_link(
                    path,
                    Path::new(
                        entry
                            .link_target
                            .as_ref()
                            .context("missing symlink target")?,
                    ),
                )?;
            }
        }
        total_bytes = total_bytes
            .checked_add(entry.size)
            .context("archive size overflow")?;
    }
    ensure!(
        total_bytes == manifest.total_bytes,
        "manifest byte count mismatch"
    );
    for entry in &manifest.entries {
        let mut parent = Path::new(&entry.path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            if let Some(parent_entry) = entries.get(path) {
                ensure!(
                    parent_entry.kind == EntryKind::Directory,
                    "entry has a non-directory ancestor"
                );
            }
            parent = path.parent();
        }
        if entry.kind == EntryKind::Symlink {
            validate_resolved_link(Path::new(&entry.path), &entries)?;
        }
    }
    Ok(())
}

fn validate_resolved_link(path: &Path, entries: &BTreeMap<&Path, &EntryFingerprint>) -> Result<()> {
    use std::collections::VecDeque;
    let mut pending: VecDeque<_> = path.components().collect();
    let mut resolved = PathBuf::new();
    let mut followed = 0usize;
    while let Some(component) = pending.pop_front() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(resolved.pop(), "symlink chain escapes the archive root")
            }
            Component::Normal(name) => {
                resolved.push(name);
                if let Some(entry) = entries
                    .get(resolved.as_path())
                    .filter(|entry| entry.kind == EntryKind::Symlink)
                {
                    followed += 1;
                    ensure!(followed <= 40, "symlink chain is cyclic or too deep");
                    resolved.pop();
                    let target = Path::new(
                        entry
                            .link_target
                            .as_ref()
                            .context("symlink has no target")?,
                    );
                    for component in target.components().rev() {
                        pending.push_front(component);
                    }
                }
            }
            _ => bail!("symlink chain contains an absolute path"),
        }
    }
    Ok(())
}

fn validate_relative(path: &Path, allow_root: bool) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "empty archive path");
    ensure!(path.to_str().is_some(), "archive path is not UTF-8");
    ensure!(
        path.as_os_str().len() as u64 <= MAX_PATH_BYTES && !path.to_str().unwrap().contains('\0'),
        "archive path exceeds its limit or contains a NUL byte"
    );
    for component in path.components() {
        match component {
            Component::Normal(name) => ensure!(
                !name.to_string_lossy().contains('\\'),
                "backslash in archive path"
            ),
            Component::CurDir if allow_root && path == Path::new(".") => {}
            _ => bail!("unsafe archive path: {}", path.display()),
        }
    }
    Ok(())
}

fn validate_link(path: &Path, target: &Path) -> Result<()> {
    ensure!(!target.as_os_str().is_empty(), "empty symlink target");
    ensure!(target.to_str().is_some(), "symlink target is not UTF-8");
    let mut depth = path.parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count()
    });
    for component in target.components() {
        match component {
            Component::Normal(name) => {
                ensure!(
                    !name.to_string_lossy().contains('\\'),
                    "backslash in symlink target"
                );
                depth += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(depth > 0, "symlink points outside the archive root");
                depth -= 1;
            }
            _ => bail!("absolute symlink targets are not supported"),
        }
    }
    Ok(())
}

fn normalized_path(path: &Path) -> Result<String> {
    let result = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(
                name.to_str()
                    .map(str::to_owned)
                    .context("path is not UTF-8"),
            ),
            _ => None,
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    Ok(if result.is_empty() {
        ".".to_owned()
    } else {
        result
    })
}

fn selected_entry(path: &str, selection: &[PathBuf]) -> bool {
    selection.is_empty()
        || selection
            .iter()
            .any(|selected| selected == Path::new(".") || Path::new(path).starts_with(selected))
}

fn exclusions(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid exclusion pattern {pattern}"))?,
        );
    }
    Ok(builder.build()?)
}

fn excluded(path: &Path, exclusions: &GlobSet) -> bool {
    exclusions.is_match(path)
        || path
            .file_name()
            .is_some_and(|name| exclusions.is_match(Path::new(name)))
}

fn open_regular(path: &Path) -> Result<File> {
    let before = fs::symlink_metadata(path)?;
    ensure!(
        before.is_file(),
        "expected a regular file: {}",
        path.display()
    );
    let file = File::open(path)?;
    let opened = file.metadata()?;
    ensure!(
        same_metadata(&before, &opened),
        "file changed while opening: {}",
        path.display()
    );
    ensure!(
        same_metadata(&opened, &fs::symlink_metadata(path)?),
        "file changed while opening"
    );
    Ok(file)
}

fn same_metadata(left: &Metadata, right: &Metadata) -> bool {
    let common = left.len() == right.len()
        && left.file_type() == right.file_type()
        && left.modified().ok() == right.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        common
            && left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }
    #[cfg(not(unix))]
    common
}

fn mode(metadata: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

fn read_xattrs(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut attributes = BTreeMap::new();
    #[cfg(unix)]
    for name in xattr::list(path)
        .with_context(|| format!("cannot list extended attributes: {}", path.display()))?
    {
        let name = name
            .to_str()
            .context("extended attribute name is not UTF-8")?
            .to_owned();
        if let Some(value) = xattr::get(path, &name)? {
            attributes.insert(name, value);
        }
    }
    Ok(attributes)
}

fn restore_metadata(path: &Path, entry: &EntryFingerprint) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if entry.kind != EntryKind::Symlink {
            fs::set_permissions(path, fs::Permissions::from_mode(entry.mode & 0o777))?;
        }
        for (name, value) in &entry.xattrs {
            xattr::set(path, name, value)
                .with_context(|| format!("cannot restore extended attribute {name}"))?;
        }
    }
    let time = FileTime::from_unix_time(entry.modified_seconds, entry.modified_nanos);
    if entry.kind == EntryKind::Symlink {
        filetime::set_symlink_file_times(path, time, time)?;
    } else {
        filetime::set_file_times(path, time, time)?;
    }
    Ok(())
}

fn create_symlink(target: &str, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (target, path);
        bail!("symlink restores are unsupported on this platform")
    }
}

fn existing_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    parent
        .canonicalize()
        .context("output parent directory must exist")
}

fn sync_parent(parent: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn expect_bytes(reader: &mut dyn Read, expected: &[u8]) -> Result<()> {
    let mut actual = vec![0; expected.len()];
    reader.read_exact(&mut actual)?;
    ensure!(actual == expected, "unsupported or corrupt dcloud format");
    Ok(())
}

fn read_byte(reader: &mut dyn Read) -> Result<u8> {
    let mut byte = [0u8];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn ensure_eof(reader: &mut dyn Read) -> Result<()> {
    let mut byte = [0u8];
    ensure!(
        reader.read(&mut byte)? == 0,
        "unexpected trailing archive data"
    );
    Ok(())
}

fn copy_hash(reader: &mut dyn Read, writer: &mut dyn Write, hasher: &mut Sha256) -> Result<u64> {
    let mut buffer = [0u8; 128 * 1024];
    let mut copied = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        copied = copied
            .checked_add(read as u64)
            .context("byte count overflow")?;
    }
    Ok(copied)
}

struct HashSink<'a>(&'a mut Sha256);

struct HeaderBudget<R> {
    inner: R,
    remaining: Rc<Cell<Option<u64>>>,
}

impl<R: BufRead> Read for HeaderBudget<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let count = available.len().min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<R: BufRead> BufRead for HeaderBudget<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let remaining = self.remaining.get();
        if remaining == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "age recipient header exceeds size limit",
            ));
        }
        let available = self.inner.fill_buf()?;
        let count = remaining.map_or(available.len(), |remaining| {
            available.len().min(remaining as usize)
        });
        Ok(&available[..count])
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
        self.remaining.set(
            self.remaining
                .get()
                .map(|remaining| remaining.saturating_sub(amount as u64)),
        );
    }
}

struct BoundedWriter<'a> {
    inner: &'a mut dyn Write,
    remaining: u64,
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other(
                "archive exceeds the configured spool size limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Write for HashSink<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct HashReader<'a, R: Read> {
    input: &'a mut R,
    hasher: &'a mut Sha256,
}

impl<R: Read> Read for HashReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.input.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}

#[cfg(test)]
#[path = "../tests/unit/archive_tests.rs"]
mod tests;
