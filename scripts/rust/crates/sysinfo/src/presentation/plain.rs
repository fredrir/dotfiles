use super::branding::resolve_brand;
use crate::health::health_summary;
use crate::model::{Component, HealthIssue, RenderOptions, SystemView};

pub fn component_identity(component: &Component) -> String {
    let mut identities = vec![component.vendor.as_str(), component.model.as_str()];
    identities.extend(component.identifiers.iter().map(String::as_str));
    let brand = resolve_brand(&component.kind, &identities);
    if brand.key == component.kind {
        component.model.clone()
    } else {
        format!("{} {}", brand.name, component.model)
            .trim()
            .to_string()
    }
}
pub fn render_plain(view: &SystemView, issues: &[HealthIssue], options: RenderOptions) -> String {
    let mut lines = Vec::new();
    if options.full {
        lines.push("System".into());
        lines.push(format!("  Platform: {}", view.platform.label));
        lines.extend(
            view.system_facts
                .iter()
                .map(|fact| format!("  {}: {}", fact.label, fact.value)),
        );
        lines.push("Hardware".into());
        for component in &view.components {
            lines.push(format!(
                "  {}: {}",
                component.label,
                component_identity(component)
            ));
            lines.extend(
                component
                    .facts
                    .iter()
                    .map(|fact| format!("    {}: {}", fact.label, fact.value)),
            );
        }
        if !view.software.is_empty() {
            lines.push("Software".into());
            for badge in &view.software {
                lines.push(format!("  {}: {}", titlecase(&badge.kind), badge.label));
            }
        }
    } else {
        lines.push(format!("System: {}", view.summary.join("  ")));
        lines.extend(
            view.components
                .iter()
                .filter(|component| component.compact)
                .map(|component| format!("{}: {}", component.label, component_identity(component))),
        );
    }
    let summary = health_summary(issues);
    if !summary.is_empty() {
        lines.push(format!("Health: {summary}"));
    }
    if options.health && !issues.is_empty() {
        lines.push(String::new());
        for issue in issues {
            lines.push(format!(
                "{}: {}",
                titlecase(issue.severity.as_str()),
                issue.title
            ));
            if !issue.detail.is_empty() {
                lines.push(format!("  {}", issue.detail));
            }
            if !issue.action.is_empty() {
                lines.push(format!("  Action: {}", issue.action));
            }
        }
    }
    lines.join("\n") + "\n"
}
fn titlecase(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
