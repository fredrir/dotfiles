use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::Gitignore;

#[derive(Clone, Debug, Default)]
pub(super) struct Patterns(Vec<Arc<PatternLayer>>);

#[derive(Debug)]
pub(super) struct PatternLayer {
    pub(super) root: PathBuf,
    pub(super) matcher: Gitignore,
}

impl Patterns {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn extend(&mut self, layer: Option<Arc<PatternLayer>>) {
        if let Some(layer) = layer {
            if layer.matcher.is_empty() {
                self.0.clear();
            } else {
                self.0.push(layer);
            }
        }
    }

    pub(super) fn matches(&self, path: &Path, excluded: bool) -> Option<bool> {
        let mut matched = None;
        // A negated file cannot reopen an excluded parent directory. Match
        // parent directories too, with the most local rule winning at each
        // path. Walking explicitly also avoids ignore's outside-root panic.
        for (depth, candidate) in path.ancestors().enumerate() {
            if candidate.parent().is_none() {
                break;
            }
            let result = self.0.iter().rev().find_map(|layer| {
                if candidate == layer.root || layer.root.starts_with(candidate) {
                    return None;
                }
                let result = layer.matcher.matched(candidate, depth != 0);
                if result.is_ignore() {
                    Some(true)
                } else if result.is_whitelist() {
                    Some(false)
                } else {
                    None
                }
            });
            if let Some(result) = result {
                if !excluded || result {
                    return Some(result);
                }
                matched = Some(false);
            }
        }
        matched
    }
}
