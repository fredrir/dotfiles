use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, FirstUse, Source};
use crate::context::{Context, fingerprint};
use crate::process;

const DATA_TIMEOUT: Duration = Duration::from_secs(15);
const TEAMS_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub current: bool,
}

#[derive(Deserialize)]
struct TeamsPayload {
    teams: Vec<Team>,
}

pub struct TeamsSource {
    pub binary: PathBuf,
}

impl Source for TeamsSource {
    type Value = Vec<Team>;

    fn key(&self) -> String {
        "vercel-teams".into()
    }

    // Login, logout, and team switches change the auth config the teams are read with.
    fn stamp(&self, ctx: &Context) -> String {
        let mut stamp = fingerprint(&self.binary);
        if let Some(dir) = auth_dir(ctx) {
            for file in ["auth.json", "config.json"] {
                stamp.push('|');
                stamp.push_str(&fingerprint(&dir.join(file)));
            }
        }
        stamp
    }

    fn ttl(&self) -> Option<Duration> {
        Some(TEAMS_TTL)
    }

    fn job(&self) -> Vec<String> {
        vec!["vercel-teams".into()]
    }

    fn first_use(&self) -> FirstUse {
        FirstUse::Background
    }

    fn build(&self, ctx: &Context) -> Option<Vec<Team>> {
        if ctx.offline {
            return None;
        }
        let Some(dir) = auth_dir(ctx) else {
            return Some(Vec::new());
        };
        if !dir.join("auth.json").is_file() {
            return Some(Vec::new());
        }
        let text = process::output(
            &self.binary,
            &["teams", "ls", "--json", "--limit", "100"],
            &ctx.cwd,
            DATA_TIMEOUT,
        )?;
        let payload: TeamsPayload = serde_json::from_str(&text).ok()?;
        Some(payload.teams)
    }
}

// The data directory the CLI reads auth.json from, in its own lookup order.
pub fn auth_dir(ctx: &Context) -> Option<PathBuf> {
    let candidates = [
        ctx.var_path("XDG_DATA_HOME")
            .map(|dir| dir.join("com.vercel.cli")),
        Some(ctx.home.join(".local/share/com.vercel.cli")),
        Some(ctx.home.join("Library/Application Support/com.vercel.cli")),
        Some(ctx.home.join(".now")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| is_readable_dir(dir))
}

fn is_readable_dir(path: &Path) -> bool {
    fs::read_dir(path).is_ok()
}

pub fn teams(ctx: &Context) -> Vec<Team> {
    if ctx.offline {
        return Vec::new();
    }
    super::binary(ctx)
        .and_then(|binary| cache::load(ctx, &TeamsSource { binary }))
        .unwrap_or_default()
}
