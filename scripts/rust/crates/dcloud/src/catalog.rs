use crate::cli::Filter;
use crate::config::{Config, DestinationKind};
use crate::state::State;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub fn collect(
    config: &Config,
    state: &mut State,
    filter: &Filter,
    offline: bool,
) -> Result<Value> {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut saved = filter
        .saved
        .as_ref()
        .map(|name| {
            config
                .searches
                .get(name)
                .with_context(|| format!("unknown saved search: {name}"))
        })
        .transpose()?
        .cloned()
        .unwrap_or_default();
    if filter.host.is_some() {
        saved.host = filter.host.clone();
    }
    if filter.job.is_some() {
        saved.job = filter.job.clone();
    }
    if filter.category.is_some() {
        saved.category = filter.category.clone();
    }
    if filter.label.is_some() {
        saved.label = filter.label.clone();
    }
    if filter.search.is_some() {
        saved.text = filter.search.clone();
    }
    let destinations = filter
        .from
        .clone()
        .map(|name| vec![name])
        .unwrap_or_else(|| config.destinations.keys().cloned().collect());
    for destination in destinations {
        ensure!(
            destination == "spool" || config.destinations.contains_key(&destination),
            "unknown destination: {destination}"
        );
        if offline {
            for (_, fetched, raw) in state.cached_manifests::<Value>(&destination)? {
                let mut row = normalize(raw, &destination);
                row["cached_at"] = json!(fetched);
                row["offline"] = json!(true);
                items.push(row);
            }
            continue;
        }
        let first_item = items.len();
        let first_error = errors.len();
        for (job_name, job) in &config.jobs {
            if saved.job.as_ref().is_some_and(|name| name != job_name)
                || (destination != "spool" && !job.destinations.contains(&destination))
            {
                continue;
            }
            for host in job.sources.keys() {
                if saved.host.as_ref().is_some_and(|name| name != host)
                    || (destination == "spool" && host != &config.host)
                {
                    continue;
                }
                let result = (|| -> Result<()> {
                    let repository =
                        crate::backups::repository(config, host, job_name, &destination)?;
                    for snapshot in repository.snapshots(Some(host), Some(job_name))? {
                        let row = json!({"kind":"snapshot","id":snapshot.id,"time":snapshot.time,"host":snapshot.hostname,"job":job_name,"destination":destination,"category":snapshot.category(),"labels":snapshot.labels(),"pinned":snapshot.pinned(),"snapshot":snapshot});
                        items.push(row);
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    errors.push(format!("{destination}/{host}/{job_name}: {e:#}"));
                }
            }
        }
        if destination != "spool"
            && config
                .destinations
                .get(&destination)
                .is_some_and(|d| d.kind != DestinationKind::Rest)
        {
            match crate::objects::manifests(config, &destination, saved.host.as_deref()) {
                Ok(manifests) => {
                    for manifest in manifests {
                        items.push(normalize(serde_json::to_value(manifest)?, &destination));
                    }
                }
                Err(e) => errors.push(format!("{destination}/uploads: {e:#}")),
            }
        }
        let rows = items[first_item..]
            .iter()
            .map(|row| {
                Ok((
                    row["id"]
                        .as_str()
                        .context("catalog item has no ID")?
                        .to_string(),
                    row.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let authoritative =
            errors.len() == first_error && saved.host.is_none() && saved.job.is_none();
        cache_rows(state, &destination, &rows, authoritative)?;
    }
    items.retain(|row| {
        saved
            .host
            .as_ref()
            .is_none_or(|v| row["host"].as_str() == Some(v))
            && saved
                .job
                .as_ref()
                .is_none_or(|v| row["job"].as_str() == Some(v))
            && saved
                .category
                .as_ref()
                .is_none_or(|v| row["category"].as_str() == Some(v))
            && saved.label.as_ref().is_none_or(|label| {
                row["labels"]
                    .as_array()
                    .is_some_and(|labels| labels.iter().any(|v| v.as_str() == Some(label)))
            })
            && saved.text.as_ref().is_none_or(|text| {
                row.to_string()
                    .to_lowercase()
                    .contains(&text.to_lowercase())
            })
    });
    items.sort_by(|a, b| b["time"].as_str().cmp(&a["time"].as_str()));
    Ok(json!({"items":items,"errors":errors,"offline":offline}))
}

fn cache_rows(
    state: &mut State,
    destination: &str,
    rows: &[(String, Value)],
    authoritative: bool,
) -> Result<()> {
    if authoritative {
        state.replace_cache(destination, rows)?;
    } else {
        for (id, row) in rows {
            state.cache_manifest(destination, id, row)?;
        }
    }
    Ok(())
}

fn normalize(raw: Value, destination: &str) -> Value {
    if raw.get("format_version").is_some() {
        json!({"kind":"archive","id":raw["id"],"time":raw["created_at"],"host":raw["host"],"job":raw["job"],"destination":destination,"category":raw["category"],"labels":raw["labels"],"manifest":raw})
    } else if raw.get("kind").is_none() && raw.get("tree").is_some() {
        match serde_json::from_value::<crate::engine::Snapshot>(raw.clone()) {
            Ok(snapshot) => {
                json!({"kind":"snapshot","id":snapshot.id,"time":snapshot.time,"host":snapshot.hostname,"job":snapshot.job(),"destination":destination,"category":snapshot.category(),"labels":snapshot.labels(),"pinned":snapshot.pinned(),"snapshot":snapshot})
            }
            Err(_) => raw,
        }
    } else {
        raw
    }
}

#[cfg(test)]
#[path = "../tests/unit/catalog_tests.rs"]
mod tests;
