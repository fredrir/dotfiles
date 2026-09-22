pub mod branding;
mod hardware;
mod metrics;
mod plain;
mod pretty;
mod software;

use crate::model::{Fact, Snapshot, SystemView};
pub use plain::render_plain;
pub use pretty::{Colors, PrettyContext, render_pretty, render_with};

pub(crate) fn facts<const N: usize>(values: [(&str, String); N]) -> Vec<Fact> {
    values
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(label, value)| Fact {
            label: label.into(),
            value,
        })
        .collect()
}
pub(crate) fn joined(values: &[&str]) -> String {
    values
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("  ")
}

pub fn build_view(snapshot: &Snapshot) -> SystemView {
    let (platform, software) = software::software_badges(snapshot);
    let desktop_detected = ["DE", "WM"].iter().any(|kind| {
        snapshot
            .result(kind)
            .as_object()
            .is_some_and(|m| !m.is_empty())
    });
    let configured = ["cpu_cooler", "case", "power_supply"]
        .iter()
        .any(|key| !matches!(snapshot.configured(key), "" | "not set"));
    let mut summary = vec![platform.label.clone()];
    if snapshot.de_display != "unknown" && !snapshot.de_display.is_empty() {
        summary.push(
            snapshot.result("DE")["prettyName"]
                .as_str()
                .unwrap_or(&snapshot.de_display)
                .into(),
        );
    }
    if let Some(protocol) = snapshot.result("WM")["protocolName"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        summary.push(protocol.into());
    }
    SystemView {
        platform,
        machine_type: if desktop_detected || configured {
            "WORKSTATION"
        } else {
            "SERVER"
        }
        .into(),
        summary,
        components: hardware::hardware_components(snapshot),
        software,
        system_facts: software::system_facts(snapshot),
        gauges: metrics::gauges(snapshot),
        disks: metrics::disks(snapshot),
    }
}
