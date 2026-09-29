use super::identity::{self, Identity};
use super::{canaries, recipients, scan, sops, vault};
use crate::context::Context;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

pub fn suggested_label(context: &Context) -> String {
    if let Some(host) = crate::hosts::this(context) {
        return host;
    }
    let host = context
        .env("HOSTNAME")
        .map(|v| v.to_string_lossy().into_owned())
        .or_else(|| {
            let mut command = context.command("hostname");
            sops::capture(&mut command, 4096, "hostname")
                .ok()
                .map(|s| String::from_utf8_lossy(&s).into_owned())
        })
        .unwrap_or_else(|| "machine".into());
    let label: String = host
        .split('.')
        .next()
        .unwrap_or("machine")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .collect();
    if recipients::valid_label(&label) {
        label
    } else {
        "machine".into()
    }
}

pub fn run(context: &Context, all: bool) -> Result<ExitCode, String> {
    let recipients = recipients::load(context)?;
    let this = identity::this(context);
    let mut bad = 0;
    let mut row = |kind: &str, label: &str, detail: String| {
        println!("  {kind:<5} {label:<12} {detail}");
        if kind == "bad" {
            bad += 1;
        }
    };
    let missing: Vec<_> = ["sops", "op"]
        .into_iter()
        .filter(|p| context.program(p).is_none())
        .collect();
    row(
        if missing.is_empty() { "ok" } else { "bad" },
        "tools",
        if missing.is_empty() {
            "sops and op present".into()
        } else {
            format!("not on PATH: {}", missing.join(" "))
        },
    );
    row(
        if this.is_ok() { "ok" } else { "bad" },
        "identity",
        match &this {
            Ok((_, reference)) => reference.to_string(),
            Err(error) => error.clone(),
        },
    );
    let label = this
        .as_ref()
        .ok()
        .map(|(host, _)| host)
        .filter(|host| recipients.contains_key(*host));
    row(
        if label.is_some() { "ok" } else { "bad" },
        "enrolled",
        label
            .map(|label| format!("this machine is '{label}'"))
            .unwrap_or_else(|| {
                format!(
                    "not a recipient yet; on a machine that already decrypts: dotfile secret enroll {} <public key>",
                    suggested_label(context)
                )
            }),
    );
    row(
        if recipients.is_empty() { "bad" } else { "ok" },
        "recipients",
        format!("{} enrolled", recipients.len()),
    );
    let policy_matches = fs::read_to_string(context.root.join(".sops.yaml")).unwrap_or_default()
        == recipients::policy(&recipients);
    row(
        if policy_matches { "ok" } else { "bad" },
        ".sops.yaml",
        if policy_matches {
            "matches config/keys.dotfile".into()
        } else {
            "does not match config/keys.dotfile; run dotfile secret sync".into()
        },
    );
    let paths = scan::encrypted_paths(context)?;
    let mut locked = Vec::new();
    for path in &paths {
        if sops::decrypt(context, &context.root.join(path), &Identity::OnePassword, false).is_err() {
            locked.push(path.display().to_string());
        }
    }
    row(
        if locked.is_empty() { "ok" } else { "bad" },
        "sealed",
        format!(
            "{} encrypted, {} will not decrypt here{}",
            paths.len(),
            locked.len(),
            if all && !locked.is_empty() {
                format!(": {}", locked.join(" "))
            } else {
                String::new()
            }
        ),
    );
    let (canaries, notes) = canaries::load(context)?;
    row(
        if notes.iter().any(|n| n.contains("readable beyond")) {
            "bad"
        } else if canaries.is_empty() {
            "warn"
        } else {
            "ok"
        },
        "canaries",
        format!(
            "{} private values guarded{}",
            canaries.len(),
            if notes.is_empty() {
                String::new()
            } else {
                format!(": {}", notes.join("; "))
            }
        ),
    );
    let config = |name: &str| {
        scan::git(context, &["config", "--get", name])
            .ok()
            .map(|v| String::from_utf8_lossy(&v).trim().to_string())
            .unwrap_or_default()
    };
    let hookspath = config("core.hooksPath");
    let executable = |path: &Path| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            path.is_file()
        }
    };
    let hooks = Path::new(&hookspath)
        .file_name()
        .is_some_and(|n| n == ".githooks")
        && ["pre-commit", "pre-push"]
            .iter()
            .all(|hook| executable(&context.root.join(".githooks").join(hook)));
    row(
        if hooks { "ok" } else { "bad" },
        "hooks",
        if hooks {
            "pre-commit and pre-push active".into()
        } else {
            "core.hooksPath or executable hooks missing; run ./setup.sh".into()
        },
    );
    let cache = config("diff.sops.cachetextconv") == "true";
    let diffs = !config("diff.sops.textconv").is_empty();
    row(
        if cache {
            "bad"
        } else if diffs {
            "ok"
        } else {
            "warn"
        },
        "diffs",
        if cache {
            "cachetextconv would write plaintext into .git".into()
        } else if diffs {
            "local SOPS textconv configured".into()
        } else {
            "sops textconv not configured".into()
        },
    );
    let shared = identity::default_path(context);
    let private = vault::mode_of(&shared).is_ok_and(|mode| mode & 0o077 == 0);
    row(
        if !shared.is_file() || private { "ok" } else { "warn" },
        "shared",
        if !shared.is_file() {
            format!("{} missing; dotfile sync writes it", shared.display())
        } else if private {
            format!("{} for other projects; dotfiles never reads it", shared.display())
        } else {
            format!("{} is readable beyond this user", shared.display())
        },
    );
    if all {
        println!(
            "keys   {}\nsops   {}",
            context.root_config.join("keys.dotfile").display(),
            context.root.join(".sops.yaml").display()
        );
    }
    if bad != 0 {
        println!("{bad} of 10 checks failed");
    }
    Ok(if bad == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
