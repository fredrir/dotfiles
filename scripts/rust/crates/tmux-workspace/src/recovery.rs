use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result, config, plugins,
    tmux::Context,
    ui::{self, Choice},
};

#[derive(Default, Deserialize, Serialize)]
struct Metadata {
    schema: u32,
    snapshot_sha256: String,
    sessions: BTreeMap<String, BTreeMap<String, String>>,
    panes: BTreeMap<String, BTreeMap<String, String>>,
    views: HashSet<String>,
    #[serde(default)]
    order: BTreeMap<String, Vec<u32>>,
}

fn addresses(ctx: &Context) -> Result<HashSet<String>> {
    Ok(ctx
        .tmux
        .run(&[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index}",
        ])?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn snapshot(directory: &Path) -> Result<PathBuf> {
    let path = directory
        .join("last")
        .canonicalize()
        .map_err(|_| "no saved workspace")?;
    if path.parent() != Some(directory.canonicalize()?.as_path()) || !path.is_file() {
        return Err("saved workspace outside recovery directory".into());
    }
    Ok(path)
}

fn metadata_path(snapshot: &Path) -> PathBuf {
    snapshot.with_file_name(format!(
        "{}.workspace.json",
        snapshot.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn collect(ctx: &Context) -> Result<Metadata> {
    let mut data = Metadata {
        schema: 1,
        ..Metadata::default()
    };
    let sessions = ctx
        .tmux
        .run(&["list-sessions", "-F", "#{session_id}\t#{session_name}"])?;
    for line in sessions.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let mut options = BTreeMap::new();
        for option in [
            "@workspace-root",
            "@workspace-internal",
            "prefix",
            "prefix2",
            "status",
        ] {
            let value = ctx.tmux.run(&["show-options", "-qv", "-t", id, option])?;
            if !value.is_empty() {
                options.insert(option.into(), value);
            }
        }
        data.sessions.insert(name.into(), options);
    }
    let panes = ctx.tmux.run(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id}\t#{session_name}:#{window_index}.#{pane_index}\t#{@workspace-tool}",
    ])?;
    for line in panes.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() != 3 {
            continue;
        }
        if parts[2] == "scratch-view" {
            data.views.insert(parts[1].into());
            continue;
        }
        if let Some((window, index)) = parts[1].rsplit_once('.') {
            data.order
                .entry(window.into())
                .or_default()
                .push(index.parse()?);
        }
        let mut options = BTreeMap::new();
        for option in ["@workspace-tool", "@workspace-project", "@workspace-origin"] {
            let value = ctx
                .tmux
                .run(&["show-options", "-pqv", "-t", parts[0], option])?;
            if !value.is_empty() {
                options.insert(option.into(), value);
            }
        }
        if !options.is_empty() {
            data.panes.insert(parts[1].into(), options);
        }
    }
    for indices in data.order.values_mut() {
        indices.sort_unstable();
    }
    Ok(data)
}

pub fn run(ctx: &mut Context, restore: bool, yes: bool) -> Result<()> {
    if restore && !yes {
        let rows = vec![
            Choice::new("Cancel", "confirm", "no"),
            Choice::new(
                "Restore layouts and supported programs · existing panes preserved",
                "confirm",
                "yes",
            ),
        ];
        if !ui::choose(
            ctx,
            rows,
            "Workspace recovery",
            "Process memory, network connections and active turns are not restored",
        )?
        .is_some_and(|r| r.value == "yes")
        {
            return Ok(());
        }
    }
    let lock = plugins::Lock::read(&ctx.paths)?;
    let action = if restore { "restore" } else { "save" };
    let script = lock
        .resurrect(&ctx.paths)
        .join(format!("scripts/{action}.sh"));
    if !script.is_file() {
        return Err("resurrect unavailable; run tmux-workspace plugins install".into());
    }
    let directory = plugins::recovery_dir(ctx)?;
    let _operation = config::lock(&directory.join(".operation.lock"), false)?;
    let mut before = HashSet::new();
    let mut existing_sessions = HashSet::new();
    let mut metadata = if restore {
        let saved = snapshot(&directory)?;
        let sidecar = metadata_path(&saved);
        let metadata: Metadata = if sidecar.is_file() {
            serde_json::from_str(&fs::read_to_string(sidecar)?)?
        } else {
            Metadata::default()
        };
        if metadata.schema > 1 {
            return Err("unsupported recovery metadata".into());
        }
        if metadata.schema == 1
            && metadata.snapshot_sha256 != format!("{:x}", Sha256::digest(fs::read(&saved)?))
        {
            return Err("recovery metadata does not match snapshot".into());
        }
        let clients = ctx.tmux.clients()?;
        if clients.len() > 1
            || (clients.len() == 1 && ctx.client.as_ref().is_some_and(|c| c != &clients[0].name))
        {
            return Err("restore requires a single attached client".into());
        }
        before = addresses(ctx)?;
        existing_sessions = ctx
            .tmux
            .run(&["list-sessions", "-F", "#{session_name}"])?
            .lines()
            .map(str::to_owned)
            .collect();
        metadata
    } else {
        collect(ctx)?
    };
    ctx.tmux.set("@resurrect-never-overwrite", "on")?;
    let mut command = Command::new("bash");
    command.arg(&script);
    if !restore {
        command.arg("quiet");
    }
    plugins::external(ctx, &mut command, Some(Duration::from_secs(120)))?.checked()?;
    if restore {
        let session_rows =
            ctx.tmux
                .run(&["list-sessions", "-F", "#{session_name}\t#{session_id}"])?;
        let session_ids: BTreeMap<_, _> = session_rows
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect();
        for (session, options) in metadata.sessions {
            if existing_sessions.contains(&session) {
                continue;
            }
            let Some(target) = session_ids.get(session.as_str()) else {
                continue;
            };
            for (option, value) in options {
                if [
                    "@workspace-root",
                    "@workspace-internal",
                    "prefix",
                    "prefix2",
                    "status",
                ]
                .contains(&option.as_str())
                {
                    ctx.tmux
                        .run(&["set-option", "-t", target, &option, &value])?;
                }
            }
        }
        let restored = addresses(ctx)?;
        let pane_rows = ctx.tmux.run(&[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index}\t#{pane_id}",
        ])?;
        let pane_ids: BTreeMap<_, _> = pane_rows
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect();
        for (pane, options) in metadata.panes {
            if before.contains(&pane) {
                continue;
            }
            let Some((window, index)) = pane.rsplit_once('.') else {
                continue;
            };
            let prefix = format!("{window}.");
            let id = if !before.iter().any(|p| p.starts_with(&prefix)) {
                let mut actual: Vec<_> = pane_ids
                    .iter()
                    .filter_map(|(address, id)| {
                        address
                            .strip_prefix(&prefix)
                            .and_then(|i| i.parse::<u32>().ok())
                            .map(|i| (i, *id))
                    })
                    .collect();
                actual.sort_unstable_by_key(|p| p.0);
                metadata
                    .order
                    .get(window)
                    .and_then(|order| order.iter().position(|i| i.to_string() == index))
                    .and_then(|position| actual.get(position).map(|p| p.1))
                    .or_else(|| pane_ids.get(pane.as_str()).copied())
            } else if restored.contains(&pane) {
                pane_ids.get(pane.as_str()).copied()
            } else {
                None
            };
            let Some(id) = id else {
                continue;
            };
            for (option, value) in options {
                if ["@workspace-tool", "@workspace-project", "@workspace-origin"]
                    .contains(&option.as_str())
                {
                    ctx.tmux
                        .run(&["set-option", "-p", "-t", id, &option, &value])?;
                }
            }
        }
    } else {
        let saved = snapshot(&directory)?;
        let content = fs::read_to_string(&saved)?;
        let filtered = content
            .lines()
            .filter(|line| {
                let parts: Vec<_> = line.split('\t').collect();
                if parts.len() > 5
                    && parts[0] == "pane"
                    && metadata
                        .views
                        .contains(&format!("{}:{}.{}", parts[1], parts[2], parts[5]))
                {
                    return false;
                }
                if parts.len() > 2
                    && parts[0] == "window"
                    && !metadata
                        .order
                        .contains_key(&format!("{}:{}", parts[1], parts[2]))
                {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        metadata.snapshot_sha256 = format!("{:x}", Sha256::digest(filtered.as_bytes()));
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        file.write_all(filtered.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&saved)?;
        config::atomic_json(&metadata_path(&saved), &metadata)?;
    }
    ctx.notice(if restore {
        "Workspace restored"
    } else {
        "Workspace saved"
    });
    Ok(())
}
