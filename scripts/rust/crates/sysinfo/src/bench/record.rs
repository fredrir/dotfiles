use blake2::digest::{Update, VariableOutput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const HIB: &str = "HIB";
pub const LIB: &str = "LIB";
pub const WORLD: &str = "world";
pub const HOST: &str = "host";
pub const CLEAN: &[&str] = &["clean"];
pub const ANY: &[&str] = &["clean", "noisy", "aborted"];
pub const TIERS: [&str; 3] = ["quick", "standard", "heavy"];

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Metric {
    pub key: String,
    pub method: String,
    pub scale: String,
    pub proportion: String,
    pub comparable: String,
    pub tool: String,
    pub tool_version: String,
    pub samples: Vec<f64>,
    pub detail: Value,
}

impl Default for Metric {
    fn default() -> Self {
        Self {
            key: String::new(),
            method: String::new(),
            scale: String::new(),
            proportion: HIB.into(),
            comparable: HOST.into(),
            tool: String::new(),
            tool_version: String::new(),
            samples: Vec::new(),
            detail: json!({}),
        }
    }
}

impl Metric {
    pub fn median(&self) -> Option<f64> {
        median(&self.samples)
    }
    pub fn mad(&self) -> Option<f64> {
        let center = self.median()?;
        median(
            &self
                .samples
                .iter()
                .map(|value| (value - center).abs())
                .collect::<Vec<_>>(),
        )
    }
    pub fn rsd_pct(&self) -> f64 {
        relative_deviation(&self.samples)
    }
    pub fn family(&self) -> &str {
        self.key.split('.').next().unwrap_or("")
    }
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "key": self.key,
            "method": self.method,
            "tool": self.tool,
            "tool_version": self.tool_version,
            "scale": self.scale,
            "proportion": self.proportion,
            "comparable": self.comparable,
            "times_to_run": self.samples.len(),
            "samples": self.samples,
            "median": self.median(),
            "mad": self.mad(),
            "rsd_pct": self.rsd_pct(),
        });
        if self
            .detail
            .as_object()
            .is_some_and(|detail| !detail.is_empty())
        {
            value["detail"] = self.detail.clone();
        }
        value
    }
}

impl Serialize for Metric {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Run {
    pub run_id: String,
    pub host: String,
    pub started: String,
    pub tier: String,
    pub grade: String,
    pub snapshot: Value,
    pub install: Value,
    pub conditions: Value,
    pub metrics: Vec<Metric>,
    pub note: String,
    pub tags: Vec<String>,
    pub dotfiles_sha: String,
    pub gate_reasons: Vec<String>,
    pub bytes_written: u64,
    pub schema: u32,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            host: String::new(),
            started: String::new(),
            tier: String::new(),
            grade: String::new(),
            snapshot: json!({}),
            install: json!({}),
            conditions: json!({}),
            metrics: Vec::new(),
            note: String::new(),
            tags: Vec::new(),
            dotfiles_sha: String::new(),
            gate_reasons: Vec::new(),
            bytes_written: 0,
            schema: 1,
        }
    }
}

impl Run {
    pub fn epoch(&self) -> String {
        epoch_of(&self.snapshot)
    }
    pub fn metric(&self, key: &str) -> Option<&Metric> {
        self.metrics.iter().find(|metric| metric.key == key)
    }
    pub fn os_id(&self) -> &str {
        self.install["os"].as_str().unwrap_or("")
    }
    pub fn to_json(&self) -> Value {
        json!({
            "schema": self.schema,
            "run_id": self.run_id,
            "host": self.host,
            "epoch": self.epoch(),
            "started": self.started,
            "tier": self.tier,
            "grade": self.grade,
            "note": self.note,
            "tags": self.tags,
            "dotfiles_sha": self.dotfiles_sha,
            "gate_reasons": self.gate_reasons,
            "bytes_written": self.bytes_written,
            "snapshot": self.snapshot,
            "install": self.install,
            "conditions": self.conditions,
            "metrics": self.metrics,
        })
    }
}
impl Serialize for Run {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

pub fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut values = samples.to_vec();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}
pub fn relative_deviation(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    if mean == 0.0 {
        return 0.0;
    }
    let variance = samples
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (samples.len() - 1) as f64;
    (variance.sqrt() / mean).abs() * 100.0
}
pub fn method_series(method: &str) -> String {
    let (name, version) = method.split_once('/').unwrap_or((method, ""));
    let mut parts = version.split('.');
    format!(
        "{name}/{}.{}",
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("")
    )
}
pub fn whole_gib(value: &Value) -> String {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
        .map(|n| format!("{:.0}", (n / 1073741824.0).floor()))
        .unwrap_or_default()
}
fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() || value.as_i64() == Some(0) {
            String::new()
        } else {
            value.to_string()
        }
    })
}
pub fn identity_fields(snapshot: &Value) -> Vec<String> {
    let cpu = &snapshot["cpu"];
    let mut values = vec![
        text(&cpu["model"]),
        text(&cpu["cores_physical"]),
        text(&cpu["cores_logical"]),
        whole_gib(&snapshot["memory"]["total"]),
    ];
    for (key, size) in [("gpu", "memory_total"), ("disks", "size")] {
        let mut devices = snapshot[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|device| format!("{}:{}", text(&device["name"]), whole_gib(&device[size])))
            .collect::<Vec<_>>();
        devices.sort();
        values.extend(devices);
    }
    values
}
pub fn epoch_of(snapshot: &Value) -> String {
    let mut hash = blake2::Blake2sVar::new(4).expect("valid digest length");
    hash.update(identity_fields(snapshot).join("\n").as_bytes());
    let mut bytes = [0; 4];
    hash.finalize_variable(&mut bytes)
        .expect("fixed digest buffer");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub type Change = (String, String, Value, Value);
pub fn snapshot_differences(left: &Value, right: &Value) -> Vec<Change> {
    let mut changes = Vec::new();
    for (label, key) in [("CPU", "cpu"), ("Memory", "memory")] {
        for name in ["model", "total", "modules", "cores_physical"] {
            if left[key][name] != right[key][name] {
                changes.push((
                    label.into(),
                    name.into(),
                    left[key][name].clone(),
                    right[key][name].clone(),
                ));
            }
        }
    }
    for (label, key) in [("GPU", "gpu"), ("Storage", "disks")] {
        let names = |value: &Value| {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| text(&item["name"]))
                .collect::<Vec<_>>()
        };
        let before = names(left);
        let after = names(right);
        if before != after {
            changes.push((
                label.into(),
                "name".into(),
                json!(before.join(", ")),
                json!(after.join(", ")),
            ));
        }
    }
    changes
}
