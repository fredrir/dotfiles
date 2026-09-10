use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, bail, ensure};
use configparser::ini::Ini;
use fs2::FileExt;
use hostkit::process::{self, CaptureLimits};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use zeroize::{Zeroize, Zeroizing};

use crate::config::{Config, expand, home, identifier};

const MAX_SECRET_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryBundle {
    version: u32,
    repository_password: String,
    archive_identity: String,
}

impl Drop for RecoveryBundle {
    fn drop(&mut self) {
        self.repository_password.zeroize();
        self.archive_identity.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RcloneBundle {
    version: u32,
    config: String,
}

impl Drop for RcloneBundle {
    fn drop(&mut self) {
        self.config.zeroize();
    }
}

pub struct MaterializedSecrets {
    pub config: Config,
    runtime: Option<TempDir>,
    original_rclone_hash: Option<String>,
    original_rclone_plain_hash: Option<String>,
}

impl MaterializedSecrets {
    pub fn persist(&self) -> Result<()> {
        let (Some(encrypted), Some(plain), Some(original), Some(plain_hash)) = (
            &self.config.rclone_secrets_file,
            &self.config.rclone_config_file,
            &self.original_rclone_hash,
            &self.original_rclone_plain_hash,
        ) else {
            return Ok(());
        };
        let config = Zeroizing::new(read_private(plain)?);
        if digest(&config) == *plain_hash {
            return Ok(());
        }
        ensure!(
            digest(&fs::read(encrypted)?) == *original,
            "encrypted OAuth configuration changed during this run; refusing to overwrite it"
        );
        let bundle = RcloneBundle {
            version: 1,
            config: String::from_utf8(config.to_vec())
                .context("rclone configuration is not UTF-8")?,
        };
        seal(
            encrypted,
            &Zeroizing::new(serde_json::to_vec(&bundle)?),
            Some(original),
        )
    }

    pub fn runtime_directory(&self) -> Option<&Path> {
        self.runtime.as_ref().map(TempDir::path)
    }
}

pub fn materialize(config: &Config) -> Result<MaterializedSecrets> {
    let mut runtime_config = config.clone();
    runtime_config.runtime_digest = Some(config.digest()?);
    if config.secrets_file.is_none() && config.rclone_secrets_file.is_none() {
        return Ok(MaterializedSecrets {
            config: runtime_config,
            runtime: None,
            original_rclone_hash: None,
            original_rclone_plain_hash: None,
        });
    }
    let runtime = tempfile::Builder::new()
        .prefix("dcloud-secrets-")
        .tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700))?;
    }
    if let Some(path) = &config.secrets_file {
        ensure_runtime_outside_repository(path, runtime.path())?;
        let bundle = recovery(path)?;
        runtime_config.password_file = runtime.path().join("repository.key");
        runtime_config.identity_file = runtime.path().join("archive.key");
        write_private_new(
            &runtime_config.password_file,
            bundle.repository_password.as_bytes(),
        )?;
        write_private_new(
            &runtime_config.identity_file,
            bundle.archive_identity.as_bytes(),
        )?;
        let recipient = recipient(&bundle.archive_identity)?;
        if runtime_config.recipients.is_empty() {
            runtime_config.recipients.push(recipient);
        } else {
            ensure!(
                runtime_config.recipients.contains(&recipient),
                "configured archive recipients do not include the encrypted recovery identity"
            );
        }
    }
    let mut original_rclone_hash = None;
    let mut original_rclone_plain_hash = None;
    if let Some(path) = &config.rclone_secrets_file {
        ensure_runtime_outside_repository(path, runtime.path())?;
        let target = runtime.path().join("rclone.conf");
        if path.try_exists()? {
            let ciphertext = fs::read(path)?;
            let bundle = rclone_bundle(path)?;
            write_private_new(&target, bundle.config.as_bytes())?;
            original_rclone_hash = Some(digest(&ciphertext));
            original_rclone_plain_hash = Some(digest(bundle.config.as_bytes()));
        } else {
            write_private_new(&target, b"")?;
        }
        runtime_config.rclone_config_file = Some(target);
    }
    Ok(MaterializedSecrets {
        config: runtime_config,
        runtime: Some(runtime),
        original_rclone_hash,
        original_rclone_plain_hash,
    })
}

pub fn initialize(config: &Config) -> Result<String> {
    let encrypted = config
        .secrets_file
        .as_deref()
        .context("secrets_file must name config/dcloud/recovery.sops.json")?;
    if encrypted.try_exists()? {
        return recipient(&recovery(encrypted)?.archive_identity);
    }
    let password = expand(&config.password_file)?;
    let identity = expand(&config.identity_file)?;
    let exists = (password.try_exists()?, identity.try_exists()?);
    let bundle = match exists {
        (true, true) => RecoveryBundle {
            version: 1,
            repository_password: read_repository_password(&password)?,
            archive_identity: String::from_utf8(read_private(&identity)?)?.trim().into(),
        },
        (false, false) => {
            let identity = age::x25519::Identity::generate();
            RecoveryBundle {
                version: 1,
                repository_password: format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
                archive_identity: identity.to_string().expose_secret().to_string(),
            }
        }
        _ => bail!(
            "only one existing recovery key was found; refusing to replace recovery credentials"
        ),
    };
    validate_recovery(&bundle)?;
    let public = recipient(&bundle.archive_identity)?;
    seal(
        encrypted,
        &Zeroizing::new(serde_json::to_vec(&bundle)?),
        None,
    )?;
    Ok(public)
}

pub fn import_google_client(config: &Config, encrypted_client: &Path, remote: &str) -> Result<()> {
    identifier(remote)?;
    let encrypted_config = config
        .rclone_secrets_file
        .as_deref()
        .context("rclone_secrets_file must name config/dcloud/rclone.sops.json")?;
    let bytes = decrypt(encrypted_client)?;
    let mut client: Value =
        serde_json::from_slice(&bytes).context("invalid encrypted Google client JSON")?;
    let installed = client
        .get("installed")
        .context("Google OAuth client must be a Desktop app client")?;
    let client_id = installed["client_id"]
        .as_str()
        .context("Google client_id missing")?
        .to_string();
    let client_secret = Zeroizing::new(
        installed["client_secret"]
            .as_str()
            .context("Google client_secret missing")?
            .to_string(),
    );
    ensure!(
        client_id.ends_with(".apps.googleusercontent.com")
            && !client_id.contains(['\n', '\r', '\0']),
        "invalid Google client_id"
    );
    ensure!(
        !client_secret.is_empty()
            && client_secret.len() <= 4096
            && !client_secret.contains(['\n', '\r', '\0']),
        "invalid Google client_secret"
    );
    scrub_json(&mut client);
    let existing = if encrypted_config.try_exists()? {
        Some(fs::read(encrypted_config)?)
    } else {
        None
    };
    let mut parsed = Ini::new_cs();
    if existing.is_some() {
        let bundle = rclone_bundle(encrypted_config)?;
        parsed
            .read(bundle.config.clone())
            .map_err(|_| anyhow::anyhow!("invalid encrypted rclone configuration"))?;
    }
    configure_google_remote(&mut parsed, remote, &client_id, &client_secret)?;
    let bundle = RcloneBundle {
        version: 1,
        config: parsed.writes(),
    };
    for section in parsed.get_mut_map().values_mut() {
        for value in section.values_mut().flatten() {
            value.zeroize();
        }
    }
    seal(
        encrypted_config,
        &Zeroizing::new(serde_json::to_vec(&bundle)?),
        existing.as_deref().map(digest).as_deref(),
    )
}

fn configure_google_remote(
    parsed: &mut Ini,
    remote: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<()> {
    if let Some(kind) = parsed.get(remote, "type") {
        ensure!(
            kind == "drive" && parsed.get(remote, "client_id").as_deref() == Some(client_id),
            "remote already exists with different settings; choose another remote name"
        );
    }
    parsed.set(remote, "type", Some("drive".into()));
    parsed.set(remote, "client_id", Some(client_id.to_string()));
    parsed.set(remote, "client_secret", Some(client_secret.to_string()));
    parsed.set(remote, "scope", Some("drive.file".into()));
    Ok(())
}

pub fn authorize_google(config: &Config, remote: &str) -> Result<()> {
    identifier(remote)?;
    ensure!(
        config
            .rclone_secrets_file
            .as_ref()
            .is_some_and(|path| path.is_file()),
        "import the encrypted Google client before authorization"
    );
    let runtime = materialize(config)?;
    let path = runtime
        .config
        .rclone_config_file
        .as_deref()
        .context("encrypted rclone configuration is not configured")?;
    let mut command = Command::new(&config.tools.rclone);
    command
        .args(["config", "reconnect", "--auto-confirm"])
        .arg(format!("{remote}:"))
        .env("RCLONE_CONFIG", path)
        .stdin(Stdio::null());
    let mut output = process::output(
        &mut command,
        CaptureLimits {
            stdout: 64 * 1024,
            stderr: 64 * 1024,
        },
        Duration::from_secs(600),
    )?;
    let success = output.status.success();
    let hint = oauth_error_hint(&output.stdout, &output.stderr);
    output.stdout.zeroize();
    output.stderr.zeroize();
    ensure!(
        success,
        "Google authorization failed: {hint}; retry auth-drive --authorize"
    );
    runtime.persist()
}

fn oauth_error_hint(stdout: &[u8], stderr: &[u8]) -> &'static str {
    for (pattern, hint) in [
        (
            "invalid_client",
            "invalid_client: Google rejected the client credentials; reimport the original Desktop client JSON",
        ),
        (
            "unauthorized_client",
            "unauthorized_client: this client is not allowed to use the requested OAuth flow",
        ),
        (
            "invalid_grant",
            "invalid_grant: the authorization code expired, was revoked, or was already used",
        ),
        (
            "redirect_uri_mismatch",
            "redirect_uri_mismatch: use a Google Desktop app OAuth client",
        ),
        (
            "access_denied",
            "access_denied: Google denied consent; check the app audience and permitted test users",
        ),
        (
            "invalid_scope",
            "invalid_scope: Google rejected the requested Drive scope",
        ),
        (
            "admin_policy_enforced",
            "admin_policy_enforced: the Google account administrator blocked this app",
        ),
        (
            "accessnotconfigured",
            "Drive API access is disabled for the Google Cloud project",
        ),
        (
            "service_disabled",
            "Drive API access is disabled for the Google Cloud project",
        ),
        (
            "address already in use",
            "the local OAuth callback port is already in use",
        ),
        (
            "connection refused",
            "the OAuth service or local callback connection was refused",
        ),
        ("timeout", "the OAuth request timed out"),
        ("timed out", "the OAuth request timed out"),
    ] {
        if [stdout, stderr].iter().any(|bytes| {
            bytes
                .windows(pattern.len())
                .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
        }) {
            return hint;
        }
    }
    "rclone could not finish OAuth token exchange or the browser flow was cancelled"
}

pub fn seal_google_client(plaintext: &Path, encrypted: &Path) -> Result<()> {
    ensure_runtime_outside_repository(encrypted, plaintext)?;
    ensure!(
        fs::symlink_metadata(plaintext)?.is_file(),
        "Google client input must be a regular file outside the repository"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        File::open(plaintext)?.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let bytes = Zeroizing::new(read_private(plaintext)?);
    let mut value: Value = serde_json::from_slice(&bytes).context("invalid Google client JSON")?;
    ensure!(
        value.get("installed").is_some(),
        "Google OAuth client must be a Desktop app client"
    );
    if encrypted.try_exists()? {
        let mut existing: Value = serde_json::from_slice(&decrypt(encrypted)?)?;
        let matches = value == existing;
        scrub_json(&mut value);
        scrub_json(&mut existing);
        ensure!(
            matches,
            "encrypted Google client already exists with different settings"
        );
        return Ok(());
    }
    scrub_json(&mut value);
    seal(encrypted, &bytes, None)
}

fn recovery(path: &Path) -> Result<RecoveryBundle> {
    let bundle: RecoveryBundle =
        serde_json::from_slice(&decrypt(path)?).context("invalid encrypted recovery document")?;
    validate_recovery(&bundle)?;
    Ok(bundle)
}

fn validate_recovery(bundle: &RecoveryBundle) -> Result<()> {
    ensure!(bundle.version == 1, "unsupported recovery document version");
    ensure!(
        bundle.repository_password.len() >= 32
            && !bundle.repository_password.contains(['\n', '\r', '\0']),
        "invalid repository recovery password"
    );
    recipient(&bundle.archive_identity)?;
    Ok(())
}

fn recipient(identity: &str) -> Result<String> {
    let identity: age::x25519::Identity = identity
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid archive recovery identity"))?;
    Ok(identity.to_public().to_string())
}

fn rclone_bundle(path: &Path) -> Result<RcloneBundle> {
    let bundle: RcloneBundle =
        serde_json::from_slice(&decrypt(path)?).context("invalid encrypted rclone document")?;
    ensure!(
        bundle.version == 1 && !bundle.config.is_empty(),
        "unsupported or empty rclone document"
    );
    Ok(bundle)
}

fn decrypt(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "encrypted secret must be a regular file"
    );
    ensure!(
        fs::metadata(path)?.len() <= MAX_SECRET_BYTES as u64 * 4,
        "encrypted secret exceeds size limit"
    );
    let mut command = sops_command()?;
    command
        .args(["decrypt", "--input-type", "json", "--output-type", "json"])
        .arg(path)
        .stdin(Stdio::null());
    let mut output = process::output(
        &mut command,
        CaptureLimits {
            stdout: MAX_SECRET_BYTES,
            stderr: 16 * 1024,
        },
        Duration::from_secs(60),
    )
    .context("SOPS decryption could not start")?;
    output.stderr.zeroize();
    let plaintext = Zeroizing::new(output.stdout);
    ensure!(
        output.status.success(),
        "SOPS decryption failed; verify this machine's existing dotfile age identity"
    );
    ensure!(
        !output.stdout_truncated,
        "decrypted secret exceeds size limit"
    );
    Ok(plaintext)
}

fn seal(path: &Path, plaintext: &[u8], expected_hash: Option<&str>) -> Result<()> {
    ensure!(
        plaintext.len() <= MAX_SECRET_BYTES,
        "secret document exceeds size limit"
    );
    let parent = path
        .parent()
        .context("encrypted secret path has no parent")?;
    let repository = repository_for(path)?;
    fs::create_dir_all(parent)?;
    let mut input = NamedTempFile::new()?;
    ensure_runtime_outside_repository(path, input.path())?;
    input.write_all(plaintext)?;
    input.rewind()?;
    let mut command = sops_command()?;
    command
        .current_dir(&repository)
        .args([
            "encrypt",
            "--input-type",
            "json",
            "--output-type",
            "json",
            "--filename-override",
        ])
        .arg(path)
        .stdin(input.reopen()?);
    let mut output = process::output(
        &mut command,
        CaptureLimits {
            stdout: MAX_SECRET_BYTES * 4,
            stderr: 16 * 1024,
        },
        Duration::from_secs(60),
    )
    .context("SOPS encryption could not start")?;
    output.stderr.zeroize();
    ensure!(
        output.status.success() && !output.stdout_truncated,
        "SOPS encryption failed; check the repository age recipients"
    );
    let encoded: Value =
        serde_json::from_slice(&output.stdout).context("invalid SOPS encryption response")?;
    ensure!(
        encoded.get("sops").is_some(),
        "SOPS response is not an encrypted document"
    );
    let mut staged = NamedTempFile::new_in(parent)?;
    staged.write_all(&output.stdout)?;
    staged.as_file().sync_all()?;
    let _lock = SecretLock::acquire(path)?;
    match expected_hash {
        Some(expected) => {
            ensure!(
                fs::symlink_metadata(path)?.is_file() && digest(&fs::read(path)?) == expected,
                "encrypted secret changed; refusing to overwrite it"
            );
            staged.persist(path).map_err(|error| error.error)?;
        }
        None => {
            staged
                .persist_noclobber(path)
                .map_err(|error| error.error)?;
        }
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

struct SecretLock(File);

impl SecretLock {
    fn acquire(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .context("encrypted secret has no filename")?
            .to_str()
            .context("encrypted secret filename must be UTF-8")?;
        let lock = path.with_file_name(format!(".{name}.lock"));
        ensure!(
            !fs::symlink_metadata(&lock).is_ok_and(|metadata| metadata.file_type().is_symlink()),
            "secret lock cannot be a symlink"
        );
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock)?;
        ensure!(
            file.metadata()?.is_file(),
            "secret lock must be a regular file"
        );
        file.try_lock_exclusive()
            .context("another SOPS commit is in progress; retry after it finishes")?;
        Ok(Self(file))
    }
}

impl Drop for SecretLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn sops_command() -> Result<Command> {
    let mut command = Command::new("sops");
    if std::env::var_os("SOPS_AGE_KEY_FILE").is_none() {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".config"));
        let identity = config.join("dotfile/age/keys.txt");
        if identity.is_file() {
            command.env("SOPS_AGE_KEY_FILE", identity);
        }
    }
    Ok(command)
}

fn repository_for(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    absolute
        .ancestors()
        .skip(1)
        .find(|directory| directory.join(".sops.yaml").is_file())
        .map(Path::to_path_buf)
        .context("no repository .sops.yaml found above encrypted secret")
}

fn ensure_runtime_outside_repository(encrypted: &Path, runtime: &Path) -> Result<()> {
    let repository = fs::canonicalize(repository_for(encrypted)?)?;
    ensure!(
        !fs::canonicalize(runtime)?.starts_with(repository),
        "runtime secret directory must be outside the repository"
    );
    Ok(())
}

fn read_private(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "secret input must be a regular file"
    );
    ensure!(
        fs::metadata(path)?.len() <= MAX_SECRET_BYTES as u64,
        "secret input exceeds size limit"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_SECRET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_SECRET_BYTES,
        "secret input exceeds size limit"
    );
    Ok(bytes)
}

fn read_repository_password(path: &Path) -> Result<String> {
    let text = Zeroizing::new(String::from_utf8(read_private(path)?)?);
    Ok(text.trim_end_matches(['\n', '\r']).to_string())
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn scrub_json(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(scrub_json),
        Value::Object(values) => values.values_mut().for_each(scrub_json),
        _ => {}
    }
}

#[cfg(test)]
#[path = "../tests/unit/secrets_tests.rs"]
mod tests;
