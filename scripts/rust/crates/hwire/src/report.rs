use std::time::Duration;

use serde_json::{Value, json};
use workstation::Style;

use crate::client::{Direction, Latency};
use crate::proto::Counted;
use hostkit::Route;

pub struct Run {
    pub route: Option<Route>,
    pub from: Option<String>,
    pub to: String,
    pub latency: Latency,
    pub transfers: Vec<(Direction, Counted)>,
    pub streams: usize,
}

impl Run {
    pub fn label(&self) -> &'static str {
        match self.route {
            Some(route) => route.name(),
            None => "direct",
        }
    }

    pub fn render(&self, style: &Style, this: &str, peer: &str) -> String {
        // With a route there are two addresses worth naming; without one the
        // peer above is already the address, and repeating it says nothing.
        let path = match &self.from {
            Some(from) => format!("{}  {from} → {}", self.label(), self.to),
            None => self.label().to_string(),
        };
        let mut lines = vec![format!(
            "{} {} {}  {}",
            style.bold(this),
            style.dim("→"),
            style.bold(peer),
            style.teal(&path),
        )];
        lines.push(format!(
            "  {:<10} {}  {}",
            "latency",
            style.bold(&column(&milliseconds(self.latency.percentile(50.0)))),
            style.dim(&format!(
                "min {}  p99 {}  max {}  {} samples",
                milliseconds(self.latency.min()),
                milliseconds(self.latency.percentile(99.0)),
                milliseconds(self.latency.max()),
                self.latency.samples(),
            )),
        ));
        for (direction, counted) in &self.transfers {
            lines.push(format!(
                "  {:<10} {}  {}",
                direction.name(),
                style.bold(&column(&rate(counted.bits_per_second()))),
                style.dim(&format!(
                    "{}  over {}",
                    bytes_per_second(counted),
                    seconds(counted.elapsed),
                )),
            ));
        }
        lines.join("\n")
    }

    pub fn json(&self) -> Value {
        let mut object = json!({
            "route": self.label(),
            "from": self.from,
            "to": self.to,
            "streams": self.streams,
        });

        let map = object.as_object_mut().expect("an object");
        map.insert(
            "latency_ms".into(),
            json!({
                "min": millis(self.latency.min()),
                "p50": millis(self.latency.percentile(50.0)),
                "p99": millis(self.latency.percentile(99.0)),
                "max": millis(self.latency.max()),
                "mean": millis(self.latency.mean()),
                "samples": self.latency.samples(),
            }),
        );
        for (direction, counted) in &self.transfers {
            map.insert(
                direction.name().into(),
                json!({
                    "bits_per_second": counted.bits_per_second().round() as u64,
                    "bytes": counted.bytes,
                    "seconds": counted.elapsed.as_secs_f64(),
                }),
            );
        }
        object
    }
}

fn column(value: &str) -> String {
    format!("{value:>12}")
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub fn milliseconds(duration: Duration) -> String {
    let value = millis(duration);
    if value < 1.0 {
        format!("{value:.3} ms")
    } else {
        format!("{value:.2} ms")
    }
}

pub fn rate(bits_per_second: f64) -> String {
    match bits_per_second {
        bits if bits >= 1e9 => format!("{:.2} Gbit/s", bits / 1e9),
        bits if bits >= 1e6 => format!("{:.1} Mbit/s", bits / 1e6),
        bits => format!("{:.0} kbit/s", bits / 1e3),
    }
}

fn bytes_per_second(counted: &Counted) -> String {
    if counted.elapsed.is_zero() {
        return "0 MiB/s".into();
    }
    let mebibytes = counted.bytes as f64 / counted.elapsed.as_secs_f64() / (1024.0 * 1024.0);
    if mebibytes >= 10.0 {
        format!("{mebibytes:.0} MiB/s")
    } else {
        format!("{mebibytes:.1} MiB/s")
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.2} s", duration.as_secs_f64())
}

#[cfg(test)]
#[path = "../tests/unit/report_tests.rs"]
mod tests;
