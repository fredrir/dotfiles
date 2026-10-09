use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::cache;
use crate::context::Context;
use crate::http::Request;
use crate::node::npmrc::DEFAULT_REGISTRY;
use crate::process;

const SEARCH_SIZE: &str = "25";
const SHOWN: usize = 30;
const POPULAR_TIMEOUT: Duration = Duration::from_millis(500);
const SEARCH_TIMEOUT: Duration = Duration::from_millis(750);
const SEARCH_TTL: u64 = 3600;
const VERSIONS_TIMEOUT: Duration = Duration::from_secs(4);
const VERSIONS_TTL: u64 = 3600;
const ABBREVIATED: &str = "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub name: String,
    pub description: String,
    pub version: String,
    pub downloads: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versions {
    pub tags: Vec<(String, String)>,
    // Newest first.
    pub versions: Vec<String>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    objects: Vec<SearchObject>,
}

#[derive(Deserialize)]
struct SearchObject {
    package: SearchPackage,
    #[serde(default)]
    downloads: Option<Downloads>,
}

#[derive(Deserialize)]
struct SearchPackage {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Deserialize)]
struct Downloads {
    #[serde(default)]
    monthly: u64,
}

#[derive(Deserialize)]
struct Packument {
    #[serde(rename = "dist-tags", default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    versions: IndexMap<String, IgnoredAny>,
}

// Match package names first; downloads break ties within each kind of match.
pub fn search(ctx: &Context, registry: &str, text: &str) -> Vec<Hit> {
    if text.is_empty() {
        return Vec::new();
    }
    let cached = cached_search(ctx, registry, text);
    if ctx.cached_packages {
        return rank(cached, text);
    }
    let key = search_key(registry, text);
    if let Some((hits, age)) = cache::peek_aged::<Vec<Hit>>(ctx, &key)
        && age < if hits.is_empty() { 30 } else { SEARCH_TTL }
    {
        return rank(merge(Some(hits), Some(cached)).unwrap_or_default(), text);
    }
    // npm searches words, not prefixes. Query the strongest known name too,
    // so `prett` retrieves metadata for `prettier` without a third-party service.
    let hint = rank(cached.clone(), text)
        .into_iter()
        .find(|hit| hit.name.starts_with(text) && super::catalog::position(&hit.name).is_some())
        .map(|hit| hit.name)
        .filter(|name| name != text);
    let fetched = if let Some(hint) = hint.filter(|_| registry == DEFAULT_REGISTRY) {
        thread::scope(|scope| {
            let primary = scope.spawn(|| query(ctx, registry, text, SEARCH_SIZE, SEARCH_TIMEOUT));
            let expanded = query(ctx, registry, &hint, SEARCH_SIZE, SEARCH_TIMEOUT);
            merge(primary.join().ok().flatten(), expanded)
        })
    } else {
        query(ctx, registry, text, SEARCH_SIZE, SEARCH_TIMEOUT)
    };
    if let Some(hits) = fetched {
        let hits = rank(merge(Some(hits), Some(cached)).unwrap_or_default(), text);
        cache::store(ctx, &key, &hits);
        return hits;
    }
    let cached = rank(cached, text);
    if cached.is_empty() && registry == DEFAULT_REGISTRY {
        popular_offline(ctx, text)
    } else {
        cached
    }
}

fn merge(first: Option<Vec<Hit>>, second: Option<Vec<Hit>>) -> Option<Vec<Hit>> {
    if first.is_none() && second.is_none() {
        return None;
    }
    let mut merged: Vec<Hit> = Vec::new();
    for hit in first.into_iter().chain(second).flatten() {
        match merged.iter_mut().find(|known| known.name == hit.name) {
            Some(known) => {
                known.downloads = known.downloads.max(hit.downloads);
                if known.description.is_empty() {
                    known.description = hit.description;
                }
                if known.version.is_empty() {
                    known.version = hit.version;
                }
            }
            None => merged.push(hit),
        }
    }
    Some(merged)
}

fn cached_search(ctx: &Context, registry: &str, text: &str) -> Vec<Hit> {
    let mut pool = Vec::new();
    for end in (1..=text.len())
        .rev()
        .filter(|end| text.is_char_boundary(*end))
    {
        // Retain useful results from the previous cache format, but don't let a
        // partial old response suppress a fresh lookup for an hour.
        for key in [
            search_key(registry, &text[..end]),
            format!("search-{}-{}", host(registry), &text[..end]),
        ] {
            if let Some(hits) = cache::peek::<Vec<Hit>>(ctx, &key) {
                pool = merge(Some(pool), Some(hits)).unwrap_or_default();
            }
        }
    }
    if registry == DEFAULT_REGISTRY {
        if let Some(popular) = cache::peek::<super::popular::Popular>(ctx, "node-popular") {
            pool = merge(Some(pool), Some(popular.libraries)).unwrap_or_default();
            pool = merge(Some(pool), Some(popular.tools)).unwrap_or_default();
        }
        pool = merge(Some(pool), Some(super::catalog::matching(text))).unwrap_or_default();
    }
    pool
}

pub fn rank(hits: Vec<Hit>, text: &str) -> Vec<Hit> {
    let needle = text.to_lowercase();
    let mut kept: Vec<Hit> = hits
        .into_iter()
        .filter(|hit| hit.name.to_lowercase().contains(&needle))
        .collect();
    kept.sort_by_cached_key(|hit| {
        let name = hit.name.to_lowercase();
        let unscoped = name.split_once('/').map_or(name.as_str(), |(_, name)| name);
        let relevance = if name == needle {
            0
        } else if unscoped == needle {
            1
        } else if name.starts_with(&needle) {
            2
        } else if unscoped.starts_with(&needle) {
            3
        } else {
            4
        };
        let catalog = super::catalog::position(&name);
        let familiar = catalog.is_some() || hit.downloads >= 1_000_000;
        // Unknown counts are not evidence that a package is unpopular.
        let popularity = if familiar {
            0
        } else if (1..1000).contains(&hit.downloads) {
            2
        } else {
            1
        };
        let extension = familiar && unscoped.contains(['-', '_', '.']);
        (
            relevance,
            popularity,
            extension,
            Reverse(hit.downloads),
            catalog.unwrap_or(usize::MAX),
            name,
        )
    });
    kept.truncate(SHOWN);
    kept
}

// Bun ships an index of popular package names that answers without the network.
fn popular_offline(ctx: &Context, text: &str) -> Vec<Hit> {
    let Some(bun) = ctx.which("bun") else {
        return Vec::new();
    };
    process::output(
        &bun,
        &["getcompletes", "a", text],
        &ctx.cwd,
        POPULAR_TIMEOUT,
    )
    .map(|output| {
        output
            .lines()
            .map(str::trim)
            .filter(|name| name.starts_with(text))
            .map(|name| Hit {
                name: name.to_string(),
                description: String::new(),
                version: String::new(),
                downloads: 0,
            })
            .collect()
    })
    .unwrap_or_default()
}

fn hit(package: SearchPackage, downloads: u64) -> Hit {
    Hit {
        name: package.name,
        description: package.description.unwrap_or_default(),
        version: package.version.unwrap_or_default(),
        downloads,
    }
}

pub fn query(
    ctx: &Context,
    registry: &str,
    text: &str,
    size: &str,
    timeout: Duration,
) -> Option<Vec<Hit>> {
    let url = format!("{registry}/-/v1/search");
    let response: SearchResponse = Request::get(&url)
        .query("text", text)
        .query("size", size)
        .timeout(timeout)
        .json(ctx)?;
    Some(
        response
            .objects
            .into_iter()
            .map(|object| {
                let downloads = object.downloads.map_or(0, |downloads| downloads.monthly);
                hit(object.package, downloads)
            })
            .collect(),
    )
}

pub fn versions(ctx: &Context, registry: &str, name: &str) -> Option<Versions> {
    let key = format!("versions-{}-{name}", host(registry));
    if ctx.cached_packages {
        return cache::peek(ctx, &key);
    }
    if let Some((versions, age)) = cache::peek_aged::<Versions>(ctx, &key)
        && age < VERSIONS_TTL
    {
        return Some(versions);
    }
    let url = format!("{registry}/{}", name.replacen('/', "%2f", 1));
    let fetched = Request::get(&url)
        .accept(ABBREVIATED)
        .timeout(VERSIONS_TIMEOUT)
        .json::<Packument>(ctx)
        .map(|packument| from_packument(packument.tags, packument.versions.into_keys().collect()));
    match fetched {
        Some(versions) => {
            cache::store(ctx, &key, &versions);
            Some(versions)
        }
        None => cache::peek(ctx, &key),
    }
}

pub fn from_packument(tags: BTreeMap<String, String>, versions: Vec<String>) -> Versions {
    let mut parsed: Vec<(semver::Version, String)> = Vec::new();
    let mut unparsed: Vec<String> = Vec::new();
    for version in versions {
        match semver::Version::parse(&version) {
            Ok(parsed_version) => parsed.push((parsed_version, version)),
            Err(_) => unparsed.push(version),
        }
    }
    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    let mut tags: Vec<(String, String)> = tags.into_iter().collect();
    tags.sort_by_key(|(tag, _)| tag != "latest");
    Versions {
        tags,
        versions: parsed
            .into_iter()
            .map(|(_, version)| version)
            .chain(unparsed)
            .collect(),
    }
}

// `name@partial` split at the version separator, keeping a leading scope intact.
pub fn split_version(spec: &str) -> Option<(&str, &str)> {
    let search_from = if spec.starts_with('@') {
        spec.find('/')?
    } else {
        0
    };
    let at = spec[search_from..].find('@')? + search_from;
    (at > 0).then(|| (&spec[..at], &spec[at + 1..]))
}

fn search_key(registry: &str, prefix: &str) -> String {
    format!("search-v2-{}-{prefix}", host(registry))
}

fn host(registry: &str) -> &str {
    registry
        .split_once("://")
        .map_or(registry, |(_, rest)| rest)
        .trim_end_matches('/')
}

#[cfg(test)]
#[path = "../../tests/unit/node/registry_tests.rs"]
mod tests;
