use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::controls::{self, Control, Profile};
use crate::env::Sysfs;

pub struct Guard {
    root: PathBuf,
    controls: Vec<Control>,
    expected: Profile,
    armed: bool,
}

pub fn validate_profile(controls: &[Control], profile: &Profile) -> Result<(), String> {
    let unique = controls
        .iter()
        .map(|control| &control.path)
        .collect::<BTreeSet<_>>();
    if controls.is_empty()
        || unique.len() != controls.len()
        || profile.values.len() != controls.len()
    {
        return Err(
            "tuning profile must contain every supported captured control exactly once".into(),
        );
    }
    for control in controls {
        if !controls::allowed_path(&control.path)
            || !profile
                .values
                .get(&control.path)
                .is_some_and(|value| control.choices.contains(value))
        {
            return Err(format!(
                "unsupported value or control in tuning profile: {}",
                control.path.display()
            ));
        }
        if control.path.ends_with("energy_performance_preference") {
            let governor = control.path.with_file_name("scaling_governor");
            if profile
                .values
                .get(&governor)
                .is_some_and(|value| value == "performance")
                && profile
                    .values
                    .get(&control.path)
                    .is_some_and(|value| value != "performance")
            {
                return Err(
                    "the performance governor requires the performance EPP preference".into(),
                );
            }
        }
    }
    Ok(())
}

fn validate_hardware(root: &Path, controls: &[Control]) -> Result<(), String> {
    let discovered = controls::discover(&Sysfs {
        sys: root.into(),
        dev: PathBuf::from("/dev"),
    })?;
    for control in controls {
        let current = discovered
            .controls
            .iter()
            .find(|current| current.path == control.path)
            .ok_or_else(|| {
                format!(
                    "captured control is no longer restorable: {}",
                    control.path.display()
                )
            })?;
        if current.driver != control.driver
            || !control
                .choices
                .iter()
                .all(|choice| current.choices.contains(choice))
        {
            return Err(format!(
                "supported control configuration changed: {}",
                control.path.display()
            ));
        }
    }
    Ok(())
}

fn verify(root: &Path, profile: &Profile) -> Result<(), String> {
    for (path, expected) in &profile.values {
        let actual = controls::read(root, path)?;
        if &actual != expected {
            return Err(format!(
                "{} changed: expected {expected:?}, found {actual:?}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn write_profile(root: &Path, profile: &Profile) -> Result<(), String> {
    let mut values = profile.values.iter().collect::<Vec<_>>();
    values.sort_by_key(|(path, _)| (controls::order(path), *path));
    let mut errors = Vec::new();
    // Governors precede EPP during both application and restoration: a governor
    // change may itself change EPP, so reverse-order rollback is incorrect.
    for (path, value) in values {
        match controls::read(root, path) {
            Ok(actual) if &actual == value => {}
            Ok(_) => {
                if let Err(error) = controls::write(root, path, value) {
                    errors.push(error);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if let Err(error) = verify(root, profile) {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

impl Guard {
    /// The caller holds the machine's measurement lock for this guard's lifetime.
    pub fn begin(root: &Path, controls: Vec<Control>) -> Result<Self, String> {
        let expected = controls::original_profile(&controls);
        validate_profile(&controls, &expected)?;
        validate_hardware(root, &controls)?;
        verify(root, &expected)?;
        Ok(Self {
            root: root.canonicalize().map_err(|error| error.to_string())?,
            controls,
            expected,
            armed: true,
        })
    }

    pub fn apply(&mut self, profile: &Profile) -> Result<(), String> {
        validate_profile(&self.controls, profile)?;
        validate_hardware(&self.root, &self.controls)?;
        verify(&self.root, &self.expected)?;
        self.expected = profile.clone();
        write_profile(&self.root, profile)
    }

    pub fn reset(&mut self) -> Result<(), String> {
        let mut conflicts = BTreeSet::new();
        let mut errors = Vec::new();
        for control in &self.controls {
            if let Ok(current) = controls::read(&self.root, &control.path)
                && current != control.original
                && self.expected.values.get(&control.path) != Some(&current)
            {
                errors.push(format!(
                    "{} was changed outside the tuning session; refusing to overwrite it",
                    control.path.display()
                ));
                if let Some(policy) = control.path.parent() {
                    conflicts.insert(policy.to_owned());
                }
            }
        }
        let mut original = controls::original_profile(&self.controls);
        original.values.retain(|path, _| {
            !path
                .parent()
                .is_some_and(|policy| conflicts.contains(policy))
        });
        if let Err(error) = write_profile(&self.root, &original) {
            errors.push(error);
        }
        if errors.is_empty() {
            self.expected = original;
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn complete(&mut self, retain: bool) -> Result<(), String> {
        if retain {
            if crate::bench::runner::cancelled() {
                return Err("tuning was interrupted before retaining settings".into());
            }
            verify(&self.root, &self.expected)?;
            if crate::bench::runner::cancelled() {
                return Err("tuning was interrupted before retaining settings".into());
            }
        } else {
            self.reset()?;
        }
        self.armed = false;
        Ok(())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.reset()
        {
            eprintln!(
                "tuning restoration failed: {error}; apply the checked-out profile after resolving the error"
            );
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tune/transaction_tests.rs"]
mod tests;
