use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rayon::prelude::*;
use serde::Serialize;

use crate::{
    Result,
    config::{self, clean, expand, identity},
    process,
    tmux::Context,
    ui::{self, Choice},
};

#[derive(Debug, Serialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub path: String,
}

pub fn sessions(ctx: &Context) -> Result<Vec<Session>> {
    let result = ctx.tmux.try_run(&["list-sessions", "-F", "#{session_id}\t#{session_name}\t#{session_path}\t#{@workspace-root}\t#{@workspace-internal}"])?;
    Ok(result
        .out
        .lines()
        .filter_map(|line| {
            let p: Vec<_> = line.split('\t').collect();
            (p.len() == 5 && p[4].is_empty() && !p[1].starts_with("__workspace-")).then(|| {
                Session {
                    id: p[0].into(),
                    name: p[1].into(),
                    path: if p[3].is_empty() { p[2] } else { p[3] }.into(),
                }
            })
        })
        .collect())
}

pub fn rows(ctx: &Context) -> Result<Vec<Choice>> {
    let host = config::hostname();
    let sessions = sessions(ctx)?;
    let mut rows: Vec<_> = sessions
        .iter()
        .map(|s| {
            Choice::new(
                format!("●  {host} / {}  ·  {}", s.name, s.path),
                "session",
                &s.id,
            )
        })
        .collect();
    let known: HashSet<_> = sessions
        .iter()
        .filter_map(|s| fs::canonicalize(&s.path).ok())
        .collect();
    let settings = ctx.paths.settings()?.projects;
    let limit = settings.limit.min(10000);
    let favorites = ctx.paths.config.join("favorites");
    let mut candidates: Vec<(PathBuf, &str)> = if favorites.is_file() {
        fs::read_to_string(favorites)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| (expand(l), "★"))
            .collect()
    } else {
        Vec::new()
    };
    if ctx.pane.is_some() {
        candidates.push((ctx.root(), "◆"));
    }
    candidates.extend(settings.paths.iter().map(|v| (expand(v), "◆")));
    for root in settings
        .roots
        .iter()
        .map(|v| expand(v))
        .filter(|p| p.is_dir())
    {
        candidates.push((root.clone(), "◆"));
        if settings.scan_children
            && let Ok(entries) = fs::read_dir(&root)
        {
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                })
                .collect();
            paths.sort();
            candidates.extend(paths.into_iter().take(limit).map(|p| (p, "◆")));
        }
    }
    if settings.zoxide
        && process::which("zoxide").is_some()
        && let Ok(result) = process::capture(
            Command::new("zoxide").args(["query", "--list"]),
            None,
            Some(Duration::from_secs(2)),
        )
    {
        candidates.extend(
            result
                .out
                .lines()
                .take(limit)
                .map(|p| (PathBuf::from(p), "↗")),
        );
    }
    let mut unique = BTreeMap::new();
    for (path, kind) in candidates {
        if let Ok(path) = path.canonicalize()
            && path.is_dir()
            && valid_path(&path)
        {
            unique.entry(path).or_insert(kind);
        }
    }
    if settings.worktrees && process::which("git").is_some() {
        let repos: Vec<_> = unique
            .keys()
            .filter(|p| p.join(".git").exists())
            .take(100)
            .cloned()
            .collect();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
        let trees: Vec<Vec<PathBuf>> = pool.install(|| {
            repos
                .par_iter()
                .map(|p| {
                    process::capture(
                        Command::new("git").arg("-C").arg(p).args([
                            "worktree",
                            "list",
                            "--porcelain",
                            "-z",
                        ]),
                        None,
                        Some(Duration::from_secs(2)),
                    )
                    .ok()
                    .map(|result| {
                        result
                            .out
                            .split('\0')
                            .filter_map(|v| v.strip_prefix("worktree ").map(PathBuf::from))
                            .collect()
                    })
                    .unwrap_or_default()
                })
                .collect()
        });
        for path in trees.into_iter().flatten() {
            if path.is_dir() && valid_path(&path) {
                unique.entry(path).or_insert("⑂");
            }
        }
    }
    for (path, kind) in unique {
        if !known.contains(&path) {
            rows.push(Choice::new(
                format!(
                    "{kind}  {host} / {}  ·  {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    path.display()
                ),
                "project",
                path.to_string_lossy(),
            ));
        }
    }
    Ok(rows)
}

fn valid_path(path: &Path) -> bool {
    !path.to_string_lossy().contains(['\n', '\t', '\r'])
}

pub fn session(ctx: &Context, path: &Path) -> Result<String> {
    let path = path
        .canonicalize()
        .map_err(|_| "project directory not found")?;
    if !path.is_dir() || !valid_path(&path) {
        return Err("invalid project directory".into());
    }
    let _lock = config::lock(
        &ctx.paths
            .state
            .join("locks")
            .join(format!("{}-projects.lock", identity(&ctx.tmux.socket()?))),
        true,
    )?;
    let path_text = path.to_str().ok_or("invalid project directory")?;
    let sessions = sessions(ctx)?;
    if let Some(found) = sessions
        .iter()
        .find(|s| fs::canonicalize(&s.path).is_ok_and(|p| p == path))
    {
        return Ok(found.id.clone());
    }
    let base = regex::Regex::new(r"[^a-zA-Z0-9_-]+")?
        .replace_all(
            path.file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or("workspace"),
            "-",
        )
        .trim_matches('-')
        .chars()
        .take(45)
        .collect::<String>();
    let mut name = if base.is_empty() {
        "workspace".into()
    } else {
        base
    };
    if sessions.iter().any(|s| s.name == name) {
        name.push('-');
        name.push_str(&identity(path_text)[..6]);
    }
    let result = ctx.tmux.try_run(&[
        "new-session",
        "-dP",
        "-F",
        "#{session_id}",
        "-s",
        &name,
        "-c",
        path_text,
        "-n",
        "shell",
    ])?;
    if result.code != 0 {
        if let Some(found) = self::sessions(ctx)?
            .into_iter()
            .find(|s| s.path == path_text)
        {
            return Ok(found.id);
        }
        return Err(result.err.into());
    }
    let id = result.out.trim().to_owned();
    ctx.tmux
        .run(&["set-option", "-t", &id, "@workspace-root", path_text])?;
    Ok(id)
}

pub fn choose(ctx: &mut Context) -> Result<i32> {
    let rows = rows(ctx)?;
    if let Some(row) = ui::choose(
        ctx,
        rows,
        "Workspaces",
        "● running · ★ favorite · ⑂ worktree · ↗ recent",
    )? {
        let id = if row.kind == "session" {
            row.value
        } else {
            session(ctx, Path::new(&row.value))?
        };
        if let Some(client) = &ctx.client {
            ctx.tmux.run(&["switch-client", "-c", client, "-t", &id])?;
        } else {
            return crate::clients::attach(ctx, &id, None);
        }
    }
    Ok(0)
}

pub fn favorite(ctx: &Context) -> Result<()> {
    let root = ctx.root();
    if !valid_path(&root) {
        return Err("invalid favorite path".into());
    }
    let target = ctx.paths.config.join("favorites");
    let _lock = config::lock(&ctx.paths.state.join("locks/favorites.lock"), true)?;
    config::private_dir(&ctx.paths.config)?;
    let existing = fs::read_to_string(&target).unwrap_or_default();
    if !existing.lines().any(|p| p == root.to_string_lossy()) {
        let mut output = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(target)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            writeln!(output)?;
        }
        writeln!(output, "{}", root.display())?;
    }
    ctx.notice(&format!(
        "Favorite: {}",
        clean(&root.to_string_lossy(), 600)
    ));
    Ok(())
}

pub fn enter(
    ctx: &mut Context,
    target: Option<&str>,
    name: Option<&str>,
    origin: Option<&str>,
    detach: bool,
) -> Result<i32> {
    let id = if let Some(path) = target.map(expand).filter(|p| p.is_dir()) {
        session(ctx, &path)?
    } else if let Some(name) = name.or(target) {
        if name.is_empty()
            || name.starts_with('-')
            || name.starts_with("__workspace-")
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || " _-".contains(c))
        {
            return Err("session name: use letters, digits, spaces, underscores or hyphens".into());
        }
        let _lock = config::lock(
            &ctx.paths
                .state
                .join("locks")
                .join(format!("{}-projects.lock", identity(&ctx.tmux.socket()?))),
            true,
        )?;
        if let Some(found) = sessions(ctx)?.into_iter().find(|s| s.name == name) {
            found.id
        } else {
            ctx.tmux.run(&[
                "new-session",
                "-dP",
                "-F",
                "#{session_id}",
                "-s",
                name,
                "-c",
                std::env::current_dir()?
                    .to_str()
                    .ok_or("invalid directory")?,
            ])?
        }
    } else {
        session(ctx, &std::env::current_dir()?)?
    };
    if detach {
        println!("{id}");
        return Ok(0);
    }
    if std::env::var_os("TMUX").is_some() && origin.is_none() {
        ctx.resolve()?;
        ctx.tmux
            .run(&["switch-client", "-c", ctx.client()?, "-t", &id])?;
        Ok(0)
    } else {
        crate::clients::attach(ctx, &id, origin)
    }
}
