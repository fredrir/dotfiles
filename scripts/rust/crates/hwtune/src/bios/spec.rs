use std::fs;
use std::path::Path;

use workstation::blocks::{self, Comments};

pub const LIVE_BLOCK: &str = "live";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    pub section: String,
    pub name: String,
    pub occurrence: Option<usize>,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Live {
    pub base_boost_mhz: Option<u32>,
    pub memory_mts: Option<u32>,
    pub gpu: Option<String>,
}

#[derive(Debug, Default)]
pub struct Spec {
    pub expectations: Vec<Expectation>,
    pub live: Live,
}

impl Spec {
    pub fn value(&self, name: &str) -> Option<&str> {
        self.expectations
            .iter()
            .find(|expectation| expectation.name == name)
            .map(|expectation| expectation.value.as_str())
    }
}

pub fn parse_key(key: &str) -> (String, Option<usize>) {
    if let Some((name, digits)) = key.rsplit_once('#')
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && let Ok(occurrence) = digits.parse::<usize>()
        && occurrence > 0
    {
        return (name.trim().to_string(), Some(occurrence));
    }
    (key.trim().to_string(), None)
}

pub fn parse(text: &str) -> Result<Spec, String> {
    let mut spec = Spec::default();
    for entry in blocks::parse_with_comments(text, Comments::Lines)? {
        if entry.opens {
            continue;
        }
        let (key, value) = entry.split();
        if value.is_empty() {
            return Err(format!("line {}: {key} has no value", entry.number));
        }
        if entry.block == LIVE_BLOCK {
            let number = || {
                value
                    .parse::<u32>()
                    .map_err(|_| format!("line {}: {key} is not a number", entry.number))
            };
            match key {
                "base_boost_mhz" => spec.live.base_boost_mhz = Some(number()?),
                "memory_mts" => spec.live.memory_mts = Some(number()?),
                "gpu" => spec.live.gpu = Some(value.to_string()),
                _ => return Err(format!("line {}: unknown live key {key}", entry.number)),
            }
            continue;
        }
        let (name, occurrence) = parse_key(key);
        if spec
            .expectations
            .iter()
            .any(|known| known.name == name && known.occurrence == occurrence)
        {
            return Err(format!("line {}: {key} is listed twice", entry.number));
        }
        spec.expectations.push(Expectation {
            section: entry.block.clone(),
            name,
            occurrence,
            value: value.to_string(),
            line: entry.number,
        });
    }
    Ok(spec)
}

pub fn load(path: &Path) -> Result<Spec, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "../../tests/unit/bios/spec_tests.rs"]
mod tests;
