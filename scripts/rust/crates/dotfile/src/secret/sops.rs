use crate::context::Context;
use crate::process::{self, CaptureLimits};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use zeroize::{Zeroize, Zeroizing};

pub const MAX_SECRET_BYTES: usize = 16 * 1024 * 1024;

pub fn command(context: &Context, identity: Option<&Path>) -> Command {
    let mut command = context.command("sops");
    command
        .env_remove("SOPS_AGE_KEY")
        .env_remove("SOPS_AGE_KEY_CMD")
        .current_dir(&context.root)
        .env(
            "SOPS_AGE_KEY_FILE",
            identity
                .map(Path::to_path_buf)
                .unwrap_or_else(|| super::vault::identity_path(context)),
        )
        .stdin(Stdio::null());
    command
}

pub fn capture(
    command: &mut Command,
    limit: usize,
    label: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    crate::cancel::check()?;
    let mut output = process::output(
        command,
        CaptureLimits {
            stdout: limit,
            stderr: 16 * 1024,
        },
        Duration::from_secs(60),
    )
    .map_err(|e| format!("{label} could not run: {e}"))?;
    output.stderr.zeroize();
    if !output.status.success() || output.stdout_truncated {
        output.stdout.zeroize();
        return Err(format!(
            "{label} failed; verify the tool, identity, recipients, and size limit"
        ));
    }
    Ok(Zeroizing::new(output.stdout))
}

pub fn decrypt(
    context: &Context,
    path: &Path,
    identity: Option<&Path>,
    json: bool,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut cmd = command(context, identity);
    cmd.arg("-d");
    if json {
        cmd.args(["--output-type", "json"]);
    }
    cmd.arg(path);
    capture(&mut cmd, MAX_SECRET_BYTES, "SOPS decryption")
}

pub fn encrypt(
    context: &Context,
    source: &Path,
    destination: &Path,
    policy: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let mut cmd = command(context, None);
    cmd.arg("--config")
        .arg(
            policy
                .map(Path::to_path_buf)
                .unwrap_or_else(|| context.root.join(".sops.yaml")),
        )
        .arg("--filename-override")
        .arg(destination)
        .arg("-e");
    if destination.extension().is_some_and(|ext| ext == "enc") {
        cmd.args(["--input-type", "binary", "--output-type", "binary"]);
    }
    cmd.arg(source);
    capture(&mut cmd, MAX_SECRET_BYTES * 4, "SOPS encryption").map(|v| v.to_vec())
}

pub fn public_key(context: &Context, path: &Path) -> Result<String, String> {
    let mut cmd = context.command("age-keygen");
    cmd.args(["-y"]).arg(path).stdin(Stdio::null());
    let data = capture(&mut cmd, 4096, "age-keygen")?;
    let key = String::from_utf8(data.to_vec()).map_err(|_| "age-keygen returned invalid text")?;
    let key = key.trim();
    if !super::recipients::valid_key(key) {
        return Err("not readable as an age identity".into());
    }
    Ok(key.to_string())
}

pub fn generate(context: &Context, path: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_ok() {
        return Err(format!("identity already exists: {}", path.display()));
    }
    super::vault::create_private_directories(path.parent().ok_or("identity has no parent")?)?;
    super::vault::set_mode(path.parent().ok_or("identity has no parent")?, 0o700)?;
    let mut cmd = context.command("age-keygen");
    cmd.arg("-o").arg(path).stdin(Stdio::null());
    capture(&mut cmd, 4096, "age-keygen")?;
    super::vault::set_mode(path, 0o600)
}

pub fn require_identity(context: &Context, supplied: Option<&Path>) -> Result<PathBuf, String> {
    let path = supplied
        .map(|p| super::expand(context, p))
        .unwrap_or_else(|| super::vault::identity_path(context));
    if !path.is_file() {
        return Err(format!("no such identity file: {}", path.display()));
    }
    public_key(context, &path)
        .map_err(|_| format!("not readable as an age identity: {}", path.display()))?;
    Ok(path)
}
