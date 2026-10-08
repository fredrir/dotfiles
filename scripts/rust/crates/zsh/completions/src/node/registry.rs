use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache;
use crate::context::Context;
use crate::http::Request;
use crate::node::npmrc::DEFAULT_REGISTRY;
use crate::process;

const SEARCH_SIZE: &str = "25";
const SHOWN: usize = 30;
// Prefix matches from a slower-moving index; the registry's own search matches whole words only.
const SUGGESTIONS: &str = "https://api.npms.io/v2/search/suggestions";
const DOWNLOADS: &str = "https://api.npmjs.org/downloads/point/last-month";
const POPULAR_TIMEOUT: Duration = Duration::from_millis(500);
const SEARCH_TIMEOUT: Duration = Duration::from_millis(1500);
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
struct Suggestion {
    package: SearchPackage,
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

// Packages whose name contains `text`, the most downloaded first.
pub fn search(ctx: &Context, registry: &str, text: &str) -> Vec<Hit> {
    if text.is_empty() {
        return Vec::new();
    }
    let key = search_key(registry, text);
    if let Some((hits, age)) = cache::peek_aged::<Vec<Hit>>(ctx, &key)
        && age < SEARCH_TTL
    {
        return hits;
    }
    let fetched = if registry == DEFAULT_REGISTRY {
        thread::scope(|scope| {
            let words = scope.spawn(|| query(ctx, registry, text, SEARCH_SIZE, SEARCH_TIMEOUT));
            let prefixed = suggestions(ctx, text).map(|hits| with_downloads(ctx, hits));
            merge(words.join().ok().flatten(), prefixed)
        })
    } else {
        query(ctx, registry, text, SEARCH_SIZE, SEARCH_TIMEOUT)
    };
    if let Some(hits) = fetched
        .map(|hits| rank(hits, text))
        .filter(|hits| !hits.is_empty())
    {
        cache::store(ctx, &key, &hits);
        return hits;
    }
    let earlier = (1..text.len())
        .rev()
        .filter(|end| text.is_char_boundary(*end))
        .find_map(|end| cache::peek::<Vec<Hit>>(ctx, &search_key(registry, &text[..end])))
        .map(|hits| rank(hits, text))
        .unwrap_or_default();
    if earlier.is_empty() {
        popular_offline(ctx, text)
    } else {
        earlier
    }
}

pub fn rank(hits: Vec<Hit>, text: &str) -> Vec<Hit> {
    let needle = text.to_lowercase();
    let mut kept: Vec<Hit> = hits
        .into_iter()
        .filter(|hit| hit.name.to_lowercase().contains(&needle))
        .collect();
    kept.sort_by_key(|hit| Reverse(hit.downloads));
    kept.truncate(SHOWN);
    kept
}

fn merge(first: Option<Vec<Hit>>, second: Option<Vec<Hit>>) -> Option<Vec<Hit>> {
    if first.is_none() && second.is_none() {
        return None;
    }
    let mut merged: Vec<Hit> = Vec::new();
    for hit in first.into_iter().chain(second).flatten() {
        match merged.iter_mut().find(|known| known.name == hit.name) {
            Some(known) => known.downloads = known.downloads.max(hit.downloads),
            None => merged.push(hit),
        }
    }
    Some(merged)
}

fn suggestions(ctx: &Context, text: &str) -> Option<Vec<Hit>> {
    let found: Vec<Suggestion> = Request::get(SUGGESTIONS)
        .query("q", text)
        .query("size", SEARCH_SIZE)
        .timeout(SEARCH_TIMEOUT)
        .json(ctx)?;
    Some(
        found
            .into_iter()
            .map(|suggestion| hit(suggestion.package, 0))
            .collect(),
    )
}

// Monthly downloads for unscoped names in one request; the bulk endpoint refuses scoped ones.
fn with_downloads(ctx: &Context, mut hits: Vec<Hit>) -> Vec<Hit> {
    let names: Vec<&str> = hits
        .iter()
        .map(|hit| hit.name.as_str())
        .filter(|name| !name.starts_with('@'))
        .collect();
    if names.is_empty() {
        return hits;
    }
    let url = format!("{DOWNLOADS}/{}", names.join(","));
    let Some(counts) = Request::get(&url)
        .timeout(SEARCH_TIMEOUT)
        .json::<Value>(ctx)
    else {
        return hits;
    };
    for hit in &mut hits {
        hit.downloads = download_count(&counts, &hit.name).unwrap_or(hit.downloads);
    }
    hits
}

pub fn download_count(counts: &Value, name: &str) -> Option<u64> {
    let single = counts.get("package").and_then(Value::as_str) == Some(name);
    let entry = if single { counts } else { counts.get(name)? };
    entry.get("downloads").and_then(Value::as_u64)
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
    format!("search-{}-{prefix}", host(registry))
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
