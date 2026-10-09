use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::registry::Hit;

fn names() -> &'static BTreeMap<&'static str, usize> {
    static NAMES: OnceLock<BTreeMap<&'static str, usize>> = OnceLock::new();
    NAMES.get_or_init(|| {
        include_str!("../../assets/npm-common.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .enumerate()
            .map(|(rank, name)| (name, rank))
            .collect()
    })
}

pub fn position(name: &str) -> Option<usize> {
    names().get(name).copied()
}

pub fn matching(text: &str) -> Vec<Hit> {
    let text = text.to_ascii_lowercase();
    names()
        .keys()
        .filter(|name| name.contains(&text))
        .map(|name| Hit {
            name: (*name).into(),
            description: String::new(),
            version: String::new(),
            downloads: 0,
        })
        .collect()
}
