use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{self, Store};
use crate::bios::export;
use crate::env::Sysfs;
use crate::paths::{self, Paths};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: String,
    pub path: String,
    pub content_sha256: String,
    pub settings_sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bios: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lact: Option<Source>,
    pub observed: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stability_sessions: Vec<StabilityLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tuning_session: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityLink {
    pub session: String,
    pub profile: String,
    pub result: String,
    pub completed: String,
    pub evidence_known: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StabilitySession {
    pub schema: u32,
    pub host: String,
    pub session: String,
    pub completed: String,
    pub profile: String,
    pub result: String,
    pub context: RunContext,
    pub context_unchanged: bool,
    pub evidence_known: bool,
    pub details: BTreeMap<String, String>,
    pub samples_path: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn bios_source(path: &Path, bytes: &[u8]) -> Result<Source, String> {
    let parsed = export::parse(&export::decode(bytes)?);
    if parsed.settings.is_empty() {
        return Err(format!(
            "{}: BIOS export contains no settings",
            path.display()
        ));
    }
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut settings = parsed
        .settings
        .into_iter()
        .map(|setting| {
            let index = occurrences.entry(setting.name.clone()).or_default();
            let value = (setting.name, *index, setting.value);
            *index += 1;
            value
        })
        .collect::<Vec<_>>();
    settings.sort();
    Ok(Source {
        kind: "imported-bios-export".into(),
        path: path.display().to_string(),
        content_sha256: sha256(bytes),
        settings_sha256: sha256(&serde_json::to_vec(&settings).map_err(|e| e.to_string())?),
    })
}

fn canonical_yaml(value: &serde_yaml_ng::Value) -> Result<String, String> {
    use serde_yaml_ng::Value;
    match value {
        Value::Mapping(mapping) => {
            let mut entries = mapping
                .iter()
                .map(|(key, value)| Ok((canonical_yaml(key)?, canonical_yaml(value)?)))
                .collect::<Result<Vec<_>, String>>()?;
            entries.sort();
            serde_json::to_string(&("mapping", entries)).map_err(|e| e.to_string())
        }
        Value::Sequence(values) => {
            let values = values
                .iter()
                .map(canonical_yaml)
                .collect::<Result<Vec<_>, _>>()?;
            serde_json::to_string(&("sequence", values)).map_err(|e| e.to_string())
        }
        Value::Tagged(value) => serde_json::to_string(&(
            "tagged",
            value.tag.to_string(),
            canonical_yaml(&value.value)?,
        ))
        .map_err(|e| e.to_string()),
        scalar => serde_yaml_ng::to_string(scalar).map_err(|e| e.to_string()),
    }
}

pub fn lact_source(path: &Path, bytes: &[u8]) -> Result<Source, String> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if !value.is_mapping() {
        return Err(format!(
            "{}: LACT configuration must be a mapping",
            path.display()
        ));
    }
    Ok(Source {
        kind: "lact-config-file".into(),
        path: path.display().to_string(),
        content_sha256: sha256(bytes),
        settings_sha256: sha256(canonical_yaml(&value)?.as_bytes()),
    })
}

pub fn observed_settings(sys: &Sysfs) -> BTreeMap<String, String> {
    let mut settings = BTreeMap::new();
    let root = sys.sys.join("devices/system/cpu/cpufreq");
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_prefix("policy")
                .is_some_and(|id| id.parse::<u32>().is_ok())
            {
                continue;
            }
            for field in [
                "scaling_driver",
                "scaling_governor",
                "energy_performance_preference",
                "scaling_min_freq",
                "scaling_max_freq",
            ] {
                if let Ok(value) = crate::env::read_text(&entry.path().join(field)) {
                    settings.insert(format!("cpu.{name}.{field}"), value);
                }
            }
        }
    }
    for (key, path) in [
        ("cpu.boost", root.join("boost")),
        (
            "cpu.intel_pstate.no_turbo",
            sys.sys.join("devices/system/cpu/intel_pstate/no_turbo"),
        ),
        (
            "platform.profile",
            sys.sys.join("firmware/acpi/platform_profile"),
        ),
    ] {
        if let Ok(value) = crate::env::read_text(&path) {
            settings.insert(key.into(), value);
        }
    }
    settings
}

pub fn capture_sources(paths: Option<&Paths>, sys: &Sysfs, lact: &Path) -> RunContext {
    let mut context = RunContext {
        observed: observed_settings(sys),
        ..RunContext::default()
    };
    if let Some(paths) = paths {
        match export::latest(&paths.exports_dir(), &paths.host) {
            Ok(Some(path)) => match fs::read(&path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|bytes| bios_source(&path, &bytes))
            {
                Ok(source) => context.bios = Some(source),
                Err(error) => context.warnings.push(error),
            },
            Ok(None) => {}
            Err(error) => context.warnings.push(error),
        }
    }
    match fs::read(lact) {
        Ok(bytes) => match lact_source(lact, &bytes) {
            Ok(source) => context.lact = Some(source),
            Err(error) => context.warnings.push(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => context
            .warnings
            .push(format!("{}: {error}", lact.display())),
    }
    context
}

pub fn current(host: &str) -> RunContext {
    let paths = Paths {
        root: super::hosts::inventory_context().root,
        host: host.into(),
    };
    let mut context = capture_sources(Some(&paths), &Sysfs::from_env(), &paths::lact_config());
    if let Ok(fans) = crate::profile::live_fans(&crate::profile::Roots::from_env()) {
        context.observed.insert("fans.profile".into(), fans);
    }
    if let Ok(gpu) = crate::gpu::query() {
        context
            .observed
            .insert("gpu.power_cap_w".into(), gpu.power_cap_w.to_string());
    }
    context
}

pub fn same_settings(a: &RunContext, b: &RunContext) -> bool {
    let hash = |source: &Option<Source>| source.as_ref().map(|s| s.settings_sha256.clone());
    hash(&a.bios) == hash(&b.bios)
        && hash(&a.lact) == hash(&b.lact)
        && !a.observed.is_empty()
        && a.observed == b.observed
        && a.warnings.is_empty()
        && b.warnings.is_empty()
}

pub fn link_session(
    context: &mut RunContext,
    host: &str,
    receipt: &StabilitySession,
) -> Result<(), String> {
    if receipt.schema != 1 || receipt.host != host {
        return Err(format!(
            "stability session {} belongs to another host or schema",
            receipt.session
        ));
    }
    if receipt.result == "pass" && !receipt.evidence_known {
        return Err(format!(
            "stability session {} cannot establish a pass without complete evidence",
            receipt.session
        ));
    }
    if !receipt.context_unchanged || !same_settings(context, &receipt.context) {
        return Err(format!(
            "stability session {} has different or missing observed configuration",
            receipt.session
        ));
    }
    if !context
        .stability_sessions
        .iter()
        .any(|link| link.session == receipt.session)
    {
        context.stability_sessions.push(StabilityLink {
            session: receipt.session.clone(),
            profile: receipt.profile.clone(),
            result: receipt.result.clone(),
            completed: receipt.completed.clone(),
            evidence_known: receipt.evidence_known,
        });
    }
    Ok(())
}

pub fn capture(host: &str, stability_ids: &[String]) -> Result<RunContext, String> {
    let mut context = current(host);
    let store = Store::discover();
    for session in stability_ids {
        let path = store.stability_path(host, session)?;
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let receipt: StabilitySession =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if receipt.session != *session {
            return Err(format!(
                "stability session ID does not match {}",
                path.display()
            ));
        }
        link_session(&mut context, host, &receipt)?;
    }
    Ok(context)
}

pub fn save_stability(
    store: &Store,
    receipt: &StabilitySession,
) -> Result<std::path::PathBuf, String> {
    store.initialize()?;
    let path = store.stability_path(&receipt.host, &receipt.session)?;
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|e| e.to_string())?;
    store::atomic_create(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
#[path = "../../tests/unit/bench/provenance_tests.rs"]
mod tests;
