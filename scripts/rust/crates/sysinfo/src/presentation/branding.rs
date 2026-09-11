use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize)]
pub struct BrandProfile {
    pub key: String,
    pub name: String,
    pub accent: String,
    pub mark: String,
    pub kinds: Vec<String>,
    pub aliases: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct Registry {
    brands: Vec<BrandProfile>,
    generic: BTreeMap<String, BrandProfile>,
    art: BTreeMap<String, Vec<String>>,
    header_art: BTreeMap<String, Vec<String>>,
    generic_header_art: Vec<String>,
    generic_art: BTreeMap<String, Vec<String>>,
    font: BTreeMap<char, Vec<String>>,
}
static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../assets/branding.json"))
        .expect("embedded brand registry is valid")
});
pub fn normalized(value: &str) -> String {
    value
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn resolve_brand(kind: &str, values: &[&str]) -> &'static BrandProfile {
    let haystack = format!(" {} ", normalized(&values.join(" ")));
    REGISTRY
        .brands
        .iter()
        .find(|profile| {
            profile.kinds.iter().any(|k| k == kind)
                && profile
                    .aliases
                    .iter()
                    .any(|alias| haystack.contains(&format!(" {} ", normalized(alias))))
        })
        .unwrap_or_else(|| {
            REGISTRY
                .generic
                .get(kind)
                .unwrap_or(&REGISTRY.generic["os"])
        })
}
pub fn strip_brand(model: &str, profile: &BrandProfile) -> String {
    let mut value = model.trim();
    let mut prefixes = std::iter::once(&profile.name)
        .chain(profile.aliases.iter())
        .collect::<Vec<_>>();
    prefixes.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for prefix in prefixes {
        if let Some(head) = value.get(..prefix.len())
            && head.eq_ignore_ascii_case(prefix)
            && value
                .get(prefix.len()..)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            value = value[prefix.len()..].trim();
        }
    }
    if value.is_empty() {
        model.trim().into()
    } else {
        value.into()
    }
}
pub fn illustration(profile: &BrandProfile, kind: &str) -> Vec<String> {
    REGISTRY
        .art
        .get(&format!("{}::{kind}", profile.key))
        .or_else(|| REGISTRY.generic_art.get(kind))
        .cloned()
        .unwrap_or_else(|| vec![profile.mark.clone()])
}
pub fn header_illustration(profile: &BrandProfile) -> &'static [String] {
    REGISTRY
        .header_art
        .get(&profile.key)
        .unwrap_or(&REGISTRY.generic_header_art)
}
pub fn block_text(value: &str) -> Vec<String> {
    let mut value = value.to_uppercase();
    if value.chars().count() > 12 {
        let parts = value
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let mut selected = Vec::new();
        for part in parts.into_iter().rev() {
            let mut candidate = selected.clone();
            candidate.insert(0, part);
            if candidate.join("-").len() > 12 {
                break;
            }
            selected = candidate;
        }
        value = if selected.is_empty() {
            value
                .chars()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        } else {
            selected.join("-")
        };
    }
    (0..5)
        .map(|row| {
            value
                .chars()
                .map(|c| {
                    REGISTRY.font.get(&c).unwrap_or(&REGISTRY.font[&'?'])[row]
                        .chars()
                        .map(|p| if p == '1' { '█' } else { ' ' })
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join(" ")
                .trim_end()
                .to_string()
        })
        .collect()
}
