use std::path::PathBuf;
use std::time::Instant;

use crate::env::{self, Sysfs};

pub struct Rapl {
    energy: PathBuf,
    range: u64,
}

impl Rapl {
    pub fn discover(sys: &Sysfs) -> Result<Self, String> {
        let dir = sys.sys.join("class/powercap/intel-rapl:0");
        let name = env::read_text(&dir.join("name"))?;
        if !name.starts_with("package") {
            return Err(format!("{}: {name} is not a package domain", dir.display()));
        }
        let range = env::read_number(&dir.join("max_energy_range_uj"))?;
        if range == 0 {
            return Err(format!("{}: zero energy range", dir.display()));
        }
        let energy = dir.join("energy_uj");
        env::read_number(&energy)?;
        Ok(Self { energy, range })
    }

    pub fn energy_uj(&self) -> Result<u64, String> {
        env::read_number(&self.energy)
    }

    pub fn joules(&self, before: u64, after: u64) -> f64 {
        wrapped_delta(before, after, self.range) as f64 / 1e6
    }

    pub fn measure<T>(&self, work: impl FnOnce() -> T) -> Result<(T, Energy), String> {
        let started = Instant::now();
        let before = self.energy_uj()?;
        let result = work();
        let after = self.energy_uj()?;
        let seconds = started.elapsed().as_secs_f64();
        Ok((
            result,
            Energy {
                joules: self.joules(before, after),
                seconds,
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Energy {
    pub joules: f64,
    pub seconds: f64,
}

impl Energy {
    pub fn watts(&self) -> f64 {
        if self.seconds > 0.0 {
            self.joules / self.seconds
        } else {
            0.0
        }
    }
}

pub fn wrapped_delta(before: u64, after: u64, range: u64) -> u64 {
    if after >= before {
        after - before
    } else {
        range.saturating_sub(before).saturating_add(after)
    }
}

#[cfg(test)]
#[path = "../tests/unit/power_tests.rs"]
mod tests;
