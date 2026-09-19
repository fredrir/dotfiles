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
    let path = vault::identity_path(context);
    let mine = sops::public_key(context, &path).ok();
    let mut bad = 0;
    let mut row = |kind: &str, label: &str, detail: String| {
        println!("  {kind:<5} {label:<12} {detail}");
        if kind == "bad" {
            bad += 1;
        }
    };
    let missing: Vec<_> = ["age", "age-keygen", "sops"]
        .into_iter()
        .filter(|p| context.program(p).is_none())
        .collect();
    row(
        if missing.is_empty() { "ok" } else { "bad" },
        "tools",
        if missing.is_empty() {
            "age and sops present".into()
        } else {
            format!("not on PATH: {}", missing.join(" "))
        },
    );
    let private = path.is_file() && vault::mode_of(&path).is_ok_and(|mode| mode & 0o077 == 0);
    row(
        if private && mine.is_some() {
            "ok"
        } else {
            "bad"
        },
        "identity",
        if private && mine.is_some() {
            format!("{} (0600)", path.display())
        } else {
            format!(
                "missing, unreadable, or permissive identity: {}",
                path.display()
            )
        },
    );
    let label = recipients
        .iter()
        .find(|(_, key)| Some(*key) == mine.as_ref())
        .map(|(label, _)| label);
    row(
        if label.is_some() { "ok" } else { "bad" },
        "enrolled",
        label
            .map(|label| format!("this machine is '{label}'"))
            .unwrap_or_else(|| {
                format!(
                    "not a recipient yet; on a machine that already decrypts, or here with the recovery key: dotfile secret enroll {} --using <recovery>",
                    suggested_label(context)
                )
            }),
    );
    if let Some(label) = label {
        let wrapped = super::wrap::path(context, label);
        row(
            if wrapped.is_file() { "ok" } else { "warn" },
            "wrapped",
            if wrapped.is_file() {
                wrapped
                    .strip_prefix(&context.root)
                    .unwrap_or(&wrapped)
                    .display()
                    .to_string()
            } else {
                "not wrapped; run dotfile secret wrap".into()
            },
        );
    }
    let recovery = recipients
        .keys()
        .any(|label| recipients::is_recovery(label));
    row(
        if recipients.is_empty() || !recovery {
            "bad"
        } else {
            "ok"
        },
        "recipients",
        format!(
            "{} enrolled{}",
            recipients.len(),
            if recovery {
                " including recovery"
            } else {
                ", none named recovery*"
            }
        ),
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
        if sops::decrypt(context, &context.root.join(path), None, false).is_err() {
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
    let strays = [
        context
            .root_config
            .parent()
            .unwrap_or(&context.home)
            .join("sops/age/keys.txt"),
        context
            .home
            .join("Library/Application Support/sops/age/keys.txt"),
    ];
    let found: Vec<_> = strays
        .iter()
        .filter(|p| p.is_file())
        .map(|path| {
            let other = sops::public_key(context, path).ok();
            let (kind, note) = stray_finding(&recipients, mine.as_deref(), other.as_deref());
            (kind, format!("{}: {note}", path.display()))
        })
        .collect();
    row(
        if found.is_empty() {
            "ok"
        } else if found.iter().any(|(kind, _)| *kind == "warn") {
            "warn"
        } else {
            "note"
        },
        "strays",
        if found.is_empty() {
            "no identity outside the state directory".into()
        } else {
            found
                .iter()
                .map(|(_, note)| note.as_str())
                .collect::<Vec<_>>()
                .join("; ")
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

pub(crate) fn stray_finding(
    recipients: &recipients::Recipients,
    mine: Option<&str>,
    other: Option<&str>,
) -> (&'static str, String) {
    let Some(other) = other else {
        return ("warn", "not readable as an age key".into());
    };
    if mine == Some(other) {
        return ("warn", "this machine's own key, duplicated here".into());
    }
    let Some((label, _)) = recipients.iter().find(|(_, key)| key.as_str() == other) else {
        return (
            "note",
            "not a recipient here, so it opens nothing in this repository".into(),
        );
    };
    if recipients::is_recovery(label) {
        (
            "warn",
            format!("the '{label}' key, which is meant to live off-machine"),
        )
    } else {
        (
            "warn",
            format!("the '{label}' key; that machine's identity, on the wrong machine"),
        )
    }
}
