use std::process::Command;
use std::time::Duration;

use hostkit::process::{self, CaptureLimits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Mce,
    Hardware,
    Xid,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub mce: usize,
    pub hardware: usize,
    pub xid: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.mce + self.hardware + self.xid
    }

    pub fn summary(&self) -> String {
        format!("{} mce, {} hw, {} xid", self.mce, self.hardware, self.xid)
    }

    pub fn add(&mut self, category: Category) {
        match category {
            Category::Mce => self.mce += 1,
            Category::Hardware => self.hardware += 1,
            Category::Xid => self.xid += 1,
        }
    }
}

pub fn classify(line: &str) -> Option<Category> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("nvrm: xid") {
        return Some(Category::Xid);
    }
    if lower.contains("machine check") || lower.contains("mce: [hardware error]") {
        return Some(Category::Mce);
    }
    if lower.contains("[hardware error]") {
        return Some(Category::Hardware);
    }
    None
}

pub fn counts(lines: &[String]) -> Counts {
    let mut counts = Counts::default();
    for line in lines {
        if let Some(category) = classify(line) {
            counts.add(category);
        }
    }
    counts
}

pub fn since(epoch: Option<u64>) -> Result<Vec<String>, String> {
    let mut command = Command::new("journalctl");
    command.args(["-k", "-b", "-q", "-o", "cat", "--no-pager"]);
    if let Some(epoch) = epoch {
        command.arg("-S").arg(format!("@{epoch}"));
    }
    let captured = process::output(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(10),
    )
    .map_err(|e| format!("journalctl: {e}"))?;
    if !captured.status.success() {
        return Err(format!(
            "journalctl: {}",
            String::from_utf8_lossy(&captured.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&captured.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

pub fn errors_since(epoch: Option<u64>) -> Result<Vec<String>, String> {
    Ok(since(epoch)?
        .into_iter()
        .filter(|line| classify(line).is_some())
        .collect())
}

#[cfg(test)]
#[path = "../tests/unit/journal_tests.rs"]
mod tests;
