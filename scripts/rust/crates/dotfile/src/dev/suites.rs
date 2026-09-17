use super::catalog::Catalog;

pub(super) const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("dotfile", &["dotfile-cli"]),
    ("hyprland", &["dotfile-cli"]),
    ("transcript", &["dotfile-cli"]),
    ("hwtune", &["hwtune", "bench-workloads"]),
];

pub(super) fn packages(suite: &str) -> &'static [&'static str] {
    DEPENDENCIES
        .iter()
        .find_map(|(name, packages)| (*name == suite).then_some(*packages))
        .unwrap_or_default()
}

pub(super) fn matches(suite: &str, target: &str, catalog: &Catalog) -> bool {
    suite == target
        || catalog.rust.iter().any(|package| {
            package.matches(target) && packages(suite).contains(&package.name.as_str())
        })
}
