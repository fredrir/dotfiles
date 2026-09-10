use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use hostkit::process::{self, CaptureLimits, CapturedOutput};
use hostkit::shell::quote;
use hostkit::ssh::Session;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::config::{Destination, DestinationKind, expand};

#[derive(Clone, Debug, Serialize)]
pub struct TransferReceipt {
    pub key: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectInfo {
    pub key: String,
    pub bytes: u64,
}

pub struct Store {
    destination: Destination,
    rclone: PathBuf,
    rclone_config: Option<PathBuf>,
    timeout: Duration,
}

impl Store {
    pub fn new(destination: &Destination, rclone: &Path, timeout: Duration) -> Result<Self> {
        ensure!(!timeout.is_zero(), "transport timeout must be positive");
        ensure!(
            destination.kind != DestinationKind::Rest,
            "REST destinations support backups; archives require local, SFTP, or Drive storage"
        );
        ensure!(
            !destination.location.is_empty() && !destination.location.contains(['\0', '\r', '\n']),
            "invalid storage location"
        );
        if destination.kind == DestinationKind::Sftp {
            sftp_location(&destination.location)?;
            ensure!(destination.encrypted, "SFTP storage requires encryption");
        }
        if destination.kind == DestinationKind::Local {
            ensure!(
                expand(Path::new(&destination.location))?.is_absolute(),
                "local storage path must be absolute"
            );
        }
        if destination.kind == DestinationKind::Drive {
            validate_remote(&destination.location)?;
        }
        Ok(Self {
            destination: destination.clone(),
            rclone: rclone.to_path_buf(),
            rclone_config: None,
            timeout,
        })
    }

    pub fn with_rclone_config(mut self, path: Option<PathBuf>) -> Self {
        self.rclone_config = path;
        self
    }

    pub fn put_immutable(&self, key: &str, source: &Path) -> Result<TransferReceipt> {
        validate_key(key, false)?;
        let metadata = fs::symlink_metadata(source)?;
        ensure!(metadata.is_file(), "upload source must be a regular file");
        let expected = hash_file(source)?;
        match self.destination.kind {
            DestinationKind::Local => self.put_local(key, source)?,
            DestinationKind::Sftp => self.put_sftp(key, source)?,
            DestinationKind::Drive => {
                self.rclone_command(&[
                    "copyto".into(),
                    "--immutable".into(),
                    "--ignore-existing".into(),
                    "--retries".into(),
                    "1".into(),
                    "--".into(),
                    source.as_os_str().to_owned(),
                    self.remote_key(key)?.into(),
                ])?;
            }
            DestinationKind::Rest => unreachable!(),
        }
        let mut readback = NamedTempFile::new()?;
        self.receive(key, readback.as_file_mut(), Some(metadata.len()))?;
        readback.as_file().sync_all()?;
        ensure!(
            readback.as_file().metadata()?.len() == metadata.len(),
            "uploaded object size mismatch: {key}"
        );
        ensure!(
            hash_file(readback.path())? == expected,
            "uploaded object hash mismatch; existing object was preserved: {key}"
        );
        ensure!(
            hash_file(source)? == expected,
            "upload source changed during transfer"
        );
        Ok(TransferReceipt {
            key: key.into(),
            bytes: metadata.len(),
            sha256: expected,
            verified: true,
        })
    }

    pub fn get(&self, key: &str, destination: &Path) -> Result<()> {
        self.get_limited(key, destination, 100 * 1024 * 1024 * 1024)
    }

    pub fn get_limited(&self, key: &str, destination: &Path, max_bytes: u64) -> Result<()> {
        validate_key(key, false)?;
        if let Ok(metadata) = fs::symlink_metadata(destination) {
            ensure!(metadata.is_file(), "download target must be a regular file");
        }
        let parent = destination
            .parent()
            .context("download target has no parent")?;
        let mut staged = NamedTempFile::new_in(parent)?;
        self.receive(key, staged.as_file_mut(), Some(max_bytes))?;
        staged.as_file().sync_all()?;
        staged.persist(destination).map_err(|error| error.error)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>> {
        validate_key(prefix, true)?;
        let mut found = match self.destination.kind {
            DestinationKind::Local => {
                let root = expand(Path::new(&self.destination.location))?;
                if !root.try_exists()? {
                    return Ok(Vec::new());
                }
                let directory = Dir::open_ambient_dir(root, ambient_authority())?;
                let mut found = Vec::new();
                if !prefix.is_empty() && !directory.try_exists(prefix)? {
                    return Ok(found);
                }
                if !prefix.is_empty() {
                    ensure!(
                        directory.symlink_metadata(prefix)?.is_dir(),
                        "listing prefix must be a directory"
                    );
                }
                list_local(&directory, prefix, &mut found)?;
                found
            }
            DestinationKind::Sftp => self.list_sftp(prefix)?,
            DestinationKind::Drive => {
                let output = self.rclone_listing(&[
                    "lsjson".into(),
                    "--recursive".into(),
                    "--files-only".into(),
                    "--no-modtime".into(),
                    "--no-mimetype".into(),
                    "--".into(),
                    self.remote_key(prefix)?.into(),
                ])?;
                let entries: Vec<serde_json::Value> =
                    serde_json::from_slice(&output).context("invalid storage listing")?;
                entries
                    .into_iter()
                    .map(|entry| {
                        let path = entry["Path"]
                            .as_str()
                            .context("storage entry has no path")?;
                        validate_key(path, false)?;
                        Ok(ObjectInfo {
                            key: if prefix.is_empty() {
                                path.into()
                            } else {
                                format!("{prefix}/{path}")
                            },
                            bytes: entry["Size"]
                                .as_u64()
                                .context("storage entry has invalid size")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            DestinationKind::Rest => unreachable!(),
        };
        found.retain(|entry| !entry.key.split('/').any(|p| p.starts_with(".dcloud-tmp-")));
        found.sort_by(|left, right| left.key.cmp(&right.key));
        for pair in found.windows(2) {
            ensure!(
                pair[0].key != pair[1].key,
                "storage contains duplicate object names: {}",
                pair[0].key
            );
        }
        Ok(found)
    }

    pub fn list_uploads(&self, host: Option<&str>) -> Result<Vec<ObjectInfo>> {
        if let Some(host) = host {
            crate::config::identifier(host)?;
            return self.list(&format!("{host}/uploads"));
        }
        let hosts = match self.destination.kind {
            DestinationKind::Local => {
                if !expand(Path::new(&self.destination.location))?.try_exists()? {
                    return Ok(Vec::new());
                }
                let directory = self.local(false)?;
                let mut hosts = Vec::new();
                for entry in directory.entries()? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir()
                        && let Ok(name) = entry.file_name().into_string()
                        && crate::config::identifier(&name).is_ok()
                    {
                        hosts.push(name);
                    }
                }
                hosts
            }
            DestinationKind::Sftp => {
                let (host, root) = sftp_location(&self.destination.location)?;
                let script = format!(
                    "{} cd -- {}; find . ! -name . -prune -type d -exec sh -c 'for directory do printf \"%s\\000\" \"${{directory#./}}\"; done' sh {{}} +",
                    remote_listing_parent(root, ""),
                    quote(root)
                );
                let output = self.ssh_output(host, &script, None)?;
                let text = std::str::from_utf8(&output.stdout).context("invalid host listing")?;
                text.split_terminator('\0')
                    .filter(|name| crate::config::identifier(name).is_ok())
                    .map(str::to_owned)
                    .collect()
            }
            DestinationKind::Drive => {
                let output = self.rclone_listing(&[
                    "lsjson".into(),
                    "--dirs-only".into(),
                    "--no-modtime".into(),
                    "--no-mimetype".into(),
                    "--".into(),
                    self.remote_key("")?.into(),
                ])?;
                let entries: Vec<serde_json::Value> = serde_json::from_slice(&output)?;
                entries
                    .iter()
                    .filter_map(|entry| entry["Path"].as_str())
                    .filter(|name| crate::config::identifier(name).is_ok())
                    .map(str::to_owned)
                    .collect()
            }
            DestinationKind::Rest => unreachable!(),
        };
        let mut result = Vec::new();
        for host in hosts {
            result.extend(self.list(&format!("{host}/uploads"))?);
        }
        Ok(result)
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        validate_key(key, false)?;
        ensure!(
            !self.destination.append_only,
            "append-only storage cannot delete objects"
        );
        match self.destination.kind {
            DestinationKind::Local => {
                if !expand(Path::new(&self.destination.location))?.try_exists()? {
                    return Ok(());
                }
                let directory = self.local(false)?;
                match directory.symlink_metadata(key) {
                    Ok(metadata) => ensure!(metadata.is_file(), "object must be a regular file"),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
                match directory.remove_file(key) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            DestinationKind::Sftp => {
                let (host, root) = sftp_location(&self.destination.location)?;
                let path = remote_path(root, key);
                let script = format!(
                    "{} [ ! -L {} ] || exit 65; [ -e {} ] || exit 0; [ -f {} ] || exit 65; rm -f -- {}",
                    remote_directories(root, key, false, true),
                    quote(&path),
                    quote(&path),
                    quote(&path),
                    quote(&path)
                );
                self.ssh_output(host, &script, None)?;
            }
            DestinationKind::Drive => {
                let mut command = Command::new(&self.rclone);
                if let Some(path) = &self.rclone_config {
                    command.env("RCLONE_CONFIG", path);
                }
                command
                    .args(["deletefile", "--drive-use-trash=false", "--"])
                    .arg(self.remote_key(key)?)
                    .stdin(Stdio::null());
                let output = process::output(&mut command, CaptureLimits::default(), self.timeout)?;
                if !matches!(output.status.code(), Some(3 | 4)) {
                    require_success(output, "Drive object deletion")?;
                }
            }
            DestinationKind::Rest => unreachable!(),
        }
        Ok(())
    }

    fn local(&self, create: bool) -> Result<Dir> {
        let root = expand(Path::new(&self.destination.location))?;
        if create {
            fs::create_dir_all(&root)?;
        }
        ensure!(
            !fs::symlink_metadata(&root)?.file_type().is_symlink(),
            "storage root cannot be a symlink"
        );
        Dir::open_ambient_dir(root, ambient_authority()).context("storage root unavailable")
    }

    fn put_local(&self, key: &str, source: &Path) -> Result<()> {
        let directory = self.local(true)?;
        let parent = Path::new(key).parent().unwrap_or(Path::new(""));
        if !parent.as_os_str().is_empty() {
            directory.create_dir_all(parent)?;
        }
        let temporary = parent.join(format!(".dcloud-tmp-{}", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut staged = directory.open_with(&temporary, &options)?;
        let outcome = (|| -> Result<()> {
            std::io::copy(&mut File::open(source)?, &mut staged)?;
            staged.sync_all()?;
            match directory.hard_link(&temporary, &directory, key) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure!(
                        directory.symlink_metadata(key)?.is_file(),
                        "existing object is not a regular file"
                    );
                }
                Err(error) => return Err(error.into()),
            }
            directory
                .open_dir(if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                })?
                .open(".")?
                .sync_all()?;
            Ok(())
        })();
        let cleanup = directory.remove_file(&temporary);
        outcome?;
        cleanup?;
        Ok(())
    }

    fn put_sftp(&self, key: &str, source: &Path) -> Result<()> {
        let (host, root) = sftp_location(&self.destination.location)?;
        let script = upload_script(root, key)?;
        self.ssh_output(host, &script, Some(File::open(source)?))?;
        Ok(())
    }

    fn receive(&self, key: &str, destination: &mut File, max_bytes: Option<u64>) -> Result<()> {
        destination.set_len(0)?;
        destination.rewind()?;
        match self.destination.kind {
            DestinationKind::Local => {
                let directory = self.local(false)?;
                ensure!(
                    directory.symlink_metadata(key)?.is_file(),
                    "object must be a regular file"
                );
                if let Some(limit) = max_bytes {
                    ensure!(
                        directory.metadata(key)?.len() <= limit,
                        "stored object exceeds expected size"
                    );
                }
                let mut input = directory.open(key)?;
                if let Some(limit) = max_bytes {
                    std::io::copy(&mut (&mut input).take(limit), destination)?;
                    ensure!(
                        input.read(&mut [0_u8; 1])? == 0,
                        "stored object grew beyond expected size"
                    );
                } else {
                    std::io::copy(&mut input, destination)?;
                }
            }
            DestinationKind::Sftp => {
                let (host, root) = sftp_location(&self.destination.location)?;
                let path = remote_path(root, key);
                let script = format!(
                    "{} [ -f {} ] && [ ! -L {} ] || exit 66; cat -- {}",
                    remote_parent(root, key, false),
                    quote(&path),
                    quote(&path),
                    quote(&path)
                );
                let mut command = Session::new(host).batch().script(&script).command();
                command.stdin(Stdio::null());
                let output = process::output_to_file_limited(
                    &mut command,
                    destination,
                    64 * 1024,
                    max_bytes.unwrap_or(100 * 1024 * 1024 * 1024),
                    self.timeout,
                )?;
                require_success(output, "SSH download")?;
            }
            DestinationKind::Drive => {
                let mut command = Command::new(&self.rclone);
                if let Some(path) = &self.rclone_config {
                    command.env("RCLONE_CONFIG", path);
                }
                command
                    .args(["cat", "--"])
                    .arg(self.remote_key(key)?)
                    .stdin(Stdio::null());
                let output = process::output_to_file_limited(
                    &mut command,
                    destination,
                    64 * 1024,
                    max_bytes.unwrap_or(100 * 1024 * 1024 * 1024),
                    self.timeout,
                )?;
                require_success(output, "Drive download")?;
            }
            DestinationKind::Rest => unreachable!(),
        }
        destination.flush()?;
        Ok(())
    }

    fn list_sftp(&self, prefix: &str) -> Result<Vec<ObjectInfo>> {
        let (host, root) = sftp_location(&self.destination.location)?;
        let search = remote_path(root, prefix);
        let script = format!(
            "{} base={}; search={}; if [ ! -e \"$search\" ]; then exit 0; fi; [ -d \"$search\" ] && [ ! -L \"$search\" ] || exit 65; find \"$search\" -type f -exec sh -c 'base=$1; shift; for file do printf \"%s\\000\" \"${{file#\"$base\"/}}\"; wc -c < \"$file\"; printf \"\\000\"; done' sh \"$base\" {{}} +",
            remote_listing_parent(root, prefix),
            quote(root.trim_end_matches('/')),
            quote(&search),
        );
        let output = self.ssh_output(host, &script, None)?;
        let text = std::str::from_utf8(&output.stdout).context("non-UTF-8 object listing")?;
        ensure!(
            text.is_empty() || text.ends_with('\0'),
            "unterminated object listing"
        );
        let mut fields = text.split_terminator('\0');
        let mut found = Vec::new();
        while let Some(key) = fields.next() {
            validate_key(key, false)?;
            let bytes = fields
                .next()
                .context("incomplete object listing")?
                .trim()
                .parse::<u64>()?;
            ensure!(
                prefix.is_empty() || key.starts_with(&format!("{prefix}/")),
                "object listing escaped prefix"
            );
            found.push(ObjectInfo {
                key: key.into(),
                bytes,
            });
        }
        Ok(found)
    }

    fn remote_key(&self, key: &str) -> Result<String> {
        validate_key(key, true)?;
        Ok(format!(
            "{}/{}",
            self.destination.location.trim_end_matches('/'),
            key
        ))
    }

    fn ssh_output(&self, host: &str, script: &str, input: Option<File>) -> Result<CapturedOutput> {
        let mut command = Session::new(host).batch().script(script).command();
        command.stdin(input.map_or_else(Stdio::null, Stdio::from));
        run_command(&mut command, self.timeout, "SSH storage")
    }

    fn rclone_command(&self, arguments: &[std::ffi::OsString]) -> Result<CapturedOutput> {
        let mut command = Command::new(&self.rclone);
        if let Some(path) = &self.rclone_config {
            command.env("RCLONE_CONFIG", path);
        }
        run_command(
            command.args(arguments).stdin(Stdio::null()),
            self.timeout,
            "Drive storage",
        )
    }

    fn rclone_listing(&self, arguments: &[std::ffi::OsString]) -> Result<Vec<u8>> {
        let mut command = Command::new(&self.rclone);
        if let Some(path) = &self.rclone_config {
            command.env("RCLONE_CONFIG", path);
        }
        command.args(arguments).stdin(Stdio::null());
        let output = process::output(
            &mut command,
            CaptureLimits {
                stdout: 64 * 1024 * 1024,
                stderr: 256 * 1024,
            },
            self.timeout,
        )?;
        if output.status.code() == Some(3) {
            return Ok(b"[]".to_vec());
        }
        Ok(require_success(output, "Drive listing")?.stdout)
    }
}

pub(crate) fn run_command(
    command: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<CapturedOutput> {
    let output = process::output(
        command,
        CaptureLimits {
            stdout: 64 * 1024 * 1024,
            stderr: 256 * 1024,
        },
        timeout,
    )
    .with_context(|| format!("{label} command failed"))?;
    require_success(output, label)
}

fn require_success(output: CapturedOutput, label: &str) -> Result<CapturedOutput> {
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr)
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n')
            .take(4096)
            .collect::<String>();
        bail!("{label} failed ({}): {}", output.status, error.trim());
    }
    ensure!(!output.stdout_truncated, "{label} response exceeds 64 MiB");
    Ok(output)
}

pub(crate) fn validate_key(key: &str, empty: bool) -> Result<()> {
    if empty && key.is_empty() {
        return Ok(());
    }
    ensure!(!key.is_empty() && key.len() <= 2048, "invalid object key");
    ensure!(
        key.split('/').all(|component| !component.is_empty()
            && component != "."
            && component != ".."
            && component.len() <= 255
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))),
        "invalid object key: {key:?}"
    );
    Ok(())
}

pub(crate) fn validate_remote(value: &str) -> Result<(&str, &str)> {
    let (remote, path) = value
        .split_once(':')
        .context("remote path must use a configured rclone remote")?;
    ensure!(
        !remote.is_empty()
            && remote
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
        "invalid configured remote name"
    );
    ensure!(
        !path.contains(['\0', '\r', '\n']) && !path.split('/').any(|part| part == ".."),
        "invalid remote path"
    );
    Ok((remote, path))
}

fn sftp_location(value: &str) -> Result<(&str, &str)> {
    let (host, path) = value
        .split_once(':')
        .context("SFTP location must be alias:/absolute/path")?;
    ensure!(
        !host.is_empty()
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b'@')),
        "invalid SSH alias"
    );
    ensure!(
        path.starts_with('/')
            && path != "/"
            && !path.contains(['\0', '\r', '\n'])
            && !path.split('/').any(|part| part == ".." || part == "."),
        "SFTP location requires a normalized directory below root"
    );
    Ok((host, path.trim_end_matches('/')))
}

fn remote_path(root: &str, key: &str) -> String {
    if key.is_empty() {
        root.trim_end_matches('/').into()
    } else {
        format!("{}/{key}", root.trim_end_matches('/'))
    }
}

fn upload_script(root: &str, key: &str) -> Result<String> {
    validate_key(key, false)?;
    let path = remote_path(root, key);
    let parent = path
        .rsplit_once('/')
        .context("remote object has no parent")?
        .0;
    let temporary = format!("{parent}/.dcloud-tmp-{}", Uuid::new_v4());
    Ok(format!(
        "{} destination={}; temporary={}; trap 'rm -f -- \"$temporary\"' EXIT HUP INT TERM; (set -C; cat > \"$temporary\"); if [ -L \"$destination\" ]; then exit 65; fi; if ! ln -- \"$temporary\" \"$destination\" 2>/dev/null; then [ -f \"$destination\" ] && [ ! -L \"$destination\" ] || exit 65; fi",
        remote_parent(root, key, true),
        quote(&path),
        quote(&temporary),
    ))
}

fn remote_parent(root: &str, key: &str, create: bool) -> String {
    remote_directories(root, key, create, false)
}

fn remote_directories(root: &str, key: &str, create: bool, missing_empty: bool) -> String {
    let mut paths = vec![root.to_string()];
    let mut path = root.to_string();
    let parts: Vec<_> = key.split('/').collect();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        path.push('/');
        path.push_str(part);
        paths.push(path.clone());
    }
    let mut script = "set -eu; umask 077; ".to_string();
    for path in paths {
        let path = quote(&path);
        script.push_str(&format!("[ ! -L {path} ] || exit 65; "));
        if create {
            script.push_str(&format!("mkdir -p -- {path}; "));
        }
        if missing_empty {
            script.push_str(&format!("[ -e {path} ] || exit 0; "));
        }
        script.push_str(&format!("[ -d {path} ] || exit 66; "));
    }
    script
}

fn remote_listing_parent(root: &str, prefix: &str) -> String {
    remote_directories(root, prefix, false, true)
}

fn list_local(directory: &Dir, prefix: &str, found: &mut Vec<ObjectInfo>) -> Result<()> {
    ensure!(
        found.len() <= 500_000,
        "object listing exceeds 500000 entries"
    );
    let relative = if prefix.is_empty() { "." } else { prefix };
    for entry in directory.read_dir(relative)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 object name"))?;
        if name.starts_with(".dcloud-tmp-") {
            continue;
        }
        let key = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        validate_key(&key, false)?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            list_local(directory, &key, found)?;
        } else {
            ensure!(
                kind.is_file(),
                "storage contains a non-regular object: {key}"
            );
            found.push(ObjectInfo {
                key,
                bytes: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}

pub fn hash_file(path: &Path) -> Result<String> {
    hash_reader(File::open(path)?)
}

fn hash_reader(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
#[path = "../tests/unit/transport_tests.rs"]
mod tests;
