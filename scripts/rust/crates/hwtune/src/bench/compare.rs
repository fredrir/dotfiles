use super::record::{Change, LIB, Metric, Run, method_series, snapshot_differences};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Delta {
    pub key: String,
    pub scale: String,
    pub proportion: String,
    pub comparable: String,
    pub left: f64,
    pub right: f64,
    pub change_pct: f64,
    pub band_pct: f64,
    pub verdict: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub deltas: Vec<Delta>,
    pub changes: Vec<Change>,
    pub only_left: Vec<String>,
    pub only_right: Vec<String>,
}
pub fn noise_band(left: &Metric, right: &Metric) -> f64 {
    let floor = if left.samples.len() < 2 || right.samples.len() < 2 {
        8.0_f64
    } else {
        2.0_f64
    };
    [left, right]
        .iter()
        .filter_map(|m| Some((m.mad()? / m.median()?).abs() * 300.0))
        .filter(|n| n.is_finite())
        .fold(floor, f64::max)
}
pub fn blocking_reason(a: &Run, b: &Run, left: &Metric, right: &Metric) -> String {
    for (label, before, after) in [
        ("unit", &left.scale, &right.scale),
        ("direction", &left.proportion, &right.proportion),
        ("comparison scope", &left.comparable, &right.comparable),
    ] {
        if before != after {
            return format!("{label} changed: {before} vs {after}");
        }
    }
    if method_series(&left.method) != method_series(&right.method) {
        return format!("method changed: {} vs {}", left.method, right.method);
    }
    if left.family() == "workload" {
        if !a.dotfiles_sha.is_empty()
            && !b.dotfiles_sha.is_empty()
            && a.dotfiles_sha != b.dotfiles_sha
        {
            return format!(
                "configuration changed: {} vs {}",
                a.dotfiles_sha, b.dotfiles_sha
            );
        }
        if a.dotfiles_sha.ends_with("-dirty") || b.dotfiles_sha.ends_with("-dirty") {
            return "measured against an uncommitted working tree".into();
        }
    }
    if left.comparable == "host" && a.host != b.host {
        return "metric is only comparable within one machine".into();
    }
    if left.comparable == "platform" && a.os_id() != b.os_id() {
        return "metric is only comparable within one platform".into();
    }
    if left.tool != right.tool {
        return format!("different tool: {} vs {}", left.tool, right.tool);
    }
    if left.comparable == "world" && left.tool_version != right.tool_version {
        return format!(
            "different {} version: {} vs {}",
            left.tool, left.tool_version, right.tool_version
        );
    }
    String::new()
}
pub fn compare_runs(a: &Run, b: &Run) -> Comparison {
    let deltas = a
        .metrics
        .iter()
        .filter_map(|left| {
            let right = b.metric(&left.key)?;
            let before = left.median()?;
            let after = right.median()?;
            let reason = blocking_reason(a, b, left, right);
            let change = if before != 0.0 {
                (after - before) / before * 100.0
            } else {
                0.0
            };
            let band = noise_band(left, right);
            let verdict = if !reason.is_empty() {
                "blocked"
            } else if change.abs() <= band {
                "noise"
            } else if (change < 0.0) == (left.proportion == LIB) {
                "better"
            } else {
                "worse"
            };
            Some(Delta {
                key: left.key.clone(),
                scale: left.scale.clone(),
                proportion: left.proportion.clone(),
                comparable: left.comparable.clone(),
                left: before,
                right: after,
                change_pct: change,
                band_pct: band,
                verdict: verdict.into(),
                reason,
            })
        })
        .collect();
    Comparison {
        deltas,
        changes: snapshot_differences(&a.snapshot, &b.snapshot),
        only_left: a
            .metrics
            .iter()
            .filter(|m| b.metric(&m.key).is_none())
            .map(|m| m.key.clone())
            .collect(),
        only_right: b
            .metrics
            .iter()
            .filter(|m| a.metric(&m.key).is_none())
            .map(|m| m.key.clone())
            .collect(),
    }
}
pub fn regressions(comparison: &Comparison, threshold: f64) -> Vec<&Delta> {
    comparison
        .deltas
        .iter()
        .filter(|d| d.verdict == "worse" && d.change_pct.abs() >= threshold)
        .collect()
}
