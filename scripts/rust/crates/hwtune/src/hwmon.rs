use std::path::PathBuf;

use crate::env::{self, Sysfs};

pub const CHANNELS: [(&str, u8); 4] = [("radiator", 2), ("pump", 7), ("case", 3), ("vrm", 1)];
pub const CHIP: &str = "nct6799";
pub const CPU_SENSOR: &str = "k10temp";
pub const VRM_TEMP_CHANNEL: u8 = 7;

pub struct Hwmon {
    pub dir: PathBuf,
}

impl Hwmon {
    pub fn find(sys: &Sysfs, name: &str) -> Result<Self, String> {
        Ok(Self {
            dir: sys.hwmon(name)?,
        })
    }

    pub fn pwm(&self, channel: u8) -> Result<u8, String> {
        let value = env::read_number(&self.dir.join(format!("pwm{channel}")))?;
        u8::try_from(value).map_err(|_| format!("pwm{channel} out of range: {value}"))
    }

    pub fn rpm(&self, channel: u8) -> Result<u32, String> {
        let value = env::read_number(&self.dir.join(format!("fan{channel}_input")))?;
        u32::try_from(value).map_err(|_| format!("fan{channel} out of range: {value}"))
    }

    pub fn temp_c(&self, channel: u8) -> Result<f64, String> {
        let value = env::read_text(&self.dir.join(format!("temp{channel}_input")))?;
        let millidegrees: i64 = value
            .parse()
            .map_err(|_| format!("temp{channel}: not a number: {value}"))?;
        Ok(millidegrees as f64 / 1000.0)
    }

    pub fn auto_points(&self, channel: u8) -> Result<Vec<(u32, u8)>, String> {
        let mut points = Vec::new();
        for index in 1..=8 {
            let temp = self
                .dir
                .join(format!("pwm{channel}_auto_point{index}_temp"));
            if !temp.exists() {
                break;
            }
            let millidegrees = env::read_number(&temp)?;
            let pwm =
                env::read_number(&self.dir.join(format!("pwm{channel}_auto_point{index}_pwm")))?;
            let pwm =
                u8::try_from(pwm).map_err(|_| format!("auto point pwm out of range: {pwm}"))?;
            points.push((u32::try_from(millidegrees / 1000).unwrap_or(u32::MAX), pwm));
        }
        if points.is_empty() {
            return Err(format!("pwm{channel} has no auto points"));
        }
        Ok(points)
    }
}

pub fn duty_pct(pwm: u8) -> u8 {
    ((u32::from(pwm) * 100 + 127) / 255) as u8
}

pub fn duty_matches(pwm: u8, pct: u8) -> bool {
    (i32::from(duty_pct(pwm)) - i32::from(pct)).abs() <= 1
}

#[cfg(test)]
#[path = "../tests/unit/hwmon_tests.rs"]
mod tests;
