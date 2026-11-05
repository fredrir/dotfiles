use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Config;

#[derive(Clone, Debug, Default)]
pub struct OrderTable {
    orders: HashMap<String, Option<i64>>,
    source: String,
}

impl OrderTable {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, Option<i64>)>) -> Self {
        Self {
            orders: pairs.into_iter().collect(),
            source: "project".to_owned(),
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn len(&self) -> usize {
        self.orders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    pub fn order(&self, class: &str) -> Option<Option<i64>> {
        self.orders.get(class).copied()
    }

    pub(crate) fn is_known(&self, class: &str) -> bool {
        self.orders.get(class).is_some_and(Option::is_some)
    }

    pub fn insert(&mut self, class: impl Into<String>, order: Option<i64>) {
        self.orders.insert(class.into(), order);
    }

    pub fn pairs(&self) -> impl Iterator<Item = (&str, Option<i64>)> {
        self.orders
            .iter()
            .map(|(class, order)| (class.as_str(), *order))
    }
}

#[derive(Clone, Debug)]
pub struct Sorter {
    table: Arc<OrderTable>,
    config: Config,
    project_root: Option<PathBuf>,
}

impl Sorter {
    pub fn new(table: Arc<OrderTable>, config: Config) -> Self {
        Self {
            table,
            config,
            project_root: None,
        }
    }

    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    pub fn table(&self) -> &OrderTable {
        &self.table
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn sort_list(&self, list: &str) -> String {
        let tokens = crate::sorter::split_classes(list);
        if !self.config.merge || tokens.len() < 2 {
            return crate::sorter::sort_tokens(&tokens, &self.table, self.config.dedupe);
        }
        let merged: Vec<String> = match &self.project_root {
            Some(root) => match crate::bridge::merge_class_list(
                root,
                self.config.stylesheet.as_deref(),
                self.config.config_file.as_deref(),
                list,
            ) {
                Ok(classes) => crate::sorter::split_classes(&classes)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                Err(_) => crate::merge::merge_tokens(&tokens, &self.table)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
            None => crate::merge::merge_tokens(&tokens, &self.table)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let merged: Vec<&str> = merged.iter().map(String::as_str).collect();
        crate::sorter::sort_tokens(&merged, &self.table, self.config.dedupe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorter(merge: bool) -> Sorter {
        let table = OrderTable::from_pairs([
            ("p-4".to_owned(), Some(1)),
            ("p-2".to_owned(), Some(2)),
            ("custom".to_owned(), None),
        ]);
        Sorter::new(
            Arc::new(table),
            Config {
                merge,
                ..Config::default()
            },
        )
    }

    #[test]
    fn merge_is_opt_in() {
        assert_eq!(sorter(false).sort_list("p-2 p-4"), "p-4 p-2");
        assert_eq!(sorter(true).sort_list("p-2 p-4"), "p-4");
    }

    fn node_available() -> bool {
        std::process::Command::new(std::env::var_os("DOTFMT_NODE").unwrap_or_else(|| "node".into()))
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn merge_prefers_tailwind_merge_when_installed() {
        if !node_available() {
            return;
        }
        let project = tempfile::tempdir().expect("project");
        std::fs::write(
            project.path().join("package.json"),
            "{\"name\":\"fixture\"}",
        )
        .expect("package.json");
        let package = project.path().join("node_modules/tailwind-merge");
        std::fs::create_dir_all(&package).expect("tailwind-merge");
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"tailwind-merge","version":"3.0.0","main":"index.js"}"#,
        )
        .expect("package.json");
        std::fs::write(
            package.join("index.js"),
            r#"exports.twMerge = (classes) => classes.split(/\s+/).filter((name) => name !== "p-2").join(" ");"#,
        )
        .expect("index.js");
        let sorter = sorter(true).with_project_root(project.path());
        assert_eq!(sorter.sort_list("p-4 p-2"), "p-4");
    }

    #[test]
    fn merge_keeps_duplicate_unknown_classes() {
        assert_eq!(
            sorter(true).sort_list("custom custom p-2"),
            "custom custom p-2"
        );
    }
}
