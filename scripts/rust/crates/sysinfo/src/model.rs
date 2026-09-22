use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub hardware: BTreeMap<String, String>,
    pub modules: Map<String, Value>,
    pub shell_display: String,
    pub terminal_display: String,
    pub de_display: String,
    pub wm_display: String,
    pub nvidia: Vec<Value>,
    #[serde(default)]
    pub probe_errors: Vec<String>,
}

impl Snapshot {
    pub fn result(&self, kind: &str) -> &Value {
        self.modules.get(kind).unwrap_or(&Value::Null)
    }
    pub fn configured(&self, key: &str) -> &str {
        self.hardware.get(key).map(String::as_str).unwrap_or("")
    }
    pub fn is_macos(&self) -> bool {
        let os = self.result("OS");
        ["id", "name", "prettyName"].iter().any(|key| {
            let name = os[key].as_str().unwrap_or("").to_lowercase();
            name.contains("macos") || name.contains("darwin")
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}
impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}
impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthIssue {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub action: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub label: String,
    pub value: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Component {
    pub kind: String,
    pub label: String,
    pub vendor: String,
    pub model: String,
    pub art_kind: String,
    pub identifiers: Vec<String>,
    pub facts: Vec<Fact>,
    pub compact: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SoftwareBadge {
    pub kind: String,
    pub vendor: String,
    pub label: String,
    pub identifiers: Vec<String>,
}
/// A compact live metric row for the default `-p` dashboard: CPU, GPU, or RAM.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gauge {
    pub kind: String,
    pub label: String,
    pub load: Option<f64>,
    pub temperature: Option<f64>,
    pub used: Option<f64>,
    pub total: Option<f64>,
}
/// One physical disk with aggregated filesystem usage for the dashboard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskGauge {
    pub label: String,
    pub model: String,
    pub used: f64,
    pub total: f64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SystemView {
    pub platform: SoftwareBadge,
    pub machine_type: String,
    pub summary: Vec<String>,
    pub components: Vec<Component>,
    pub software: Vec<SoftwareBadge>,
    pub system_facts: Vec<Fact>,
    /// Dashboard-only metrics; excluded from the versioned JSON schema.
    #[serde(default, skip_serializing)]
    pub gauges: Vec<Gauge>,
    /// Dashboard-only disk usage; excluded from the versioned JSON schema.
    #[serde(default, skip_serializing)]
    pub disks: Vec<DiskGauge>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions {
    pub full: bool,
    pub health: bool,
}
