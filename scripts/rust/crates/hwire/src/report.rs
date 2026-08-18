//! What a measurement looks like once it is finished: one block per route for
//! a person, or one object for a program.
//!
//! Rates are printed in decimal bits per second, the unit an interface is
//! sold in, next to binary bytes per second, the unit a file copy is felt in.
//! Mixing the two bases in one line is deliberate — it is what `iperf3` and
//! every NIC datasheet do, and picking one base would make one of the two
//! numbers unrecognisable.

use std::time::Duration;

use serde_json::{Value, json};
use workstation::Style;

use crate::client::{Direction, Latency};
use crate::host::Route;
use crate::proto::Counted;

pub struct Run {
    pub route: Option<Route>,
    /// The address this side bound, when a route pinned one.
    pub from: Option<String>,
    pub to: String,
    pub latency: Latency,
    pub transfers: Vec<(Direction, Counted)>,
    pub streams: usize,
}

impl Run {
    /// The name of what was measured: the route when one was chosen, and
    /// otherwise the plain fact that an address was named.
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

/// Right-align a value before it is painted: colour is escape bytes, and a
/// width applied to those pads the wrong thing.
fn column(value: &str) -> String {
    format!("{value:>12}")
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Two decimals once past a millisecond, three below it: on the cable a round
/// trip is fractions of a millisecond, and rounding those to two decimals
/// throws away the difference between one run and the next.
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
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_keeps_the_digits_that_change() {
        assert_eq!(milliseconds(Duration::from_micros(184)), "0.184 ms");
        assert_eq!(milliseconds(Duration::from_micros(1_712)), "1.71 ms");
        assert_eq!(milliseconds(Duration::ZERO), "0.000 ms");
    }

    #[test]
    fn a_rate_carries_the_unit_it_is_worth_reading_in() {
        assert_eq!(rate(4_510_000_000.0), "4.51 Gbit/s");
        assert_eq!(rate(940_000_000.0), "940.0 Mbit/s");
        assert_eq!(rate(12_000.0), "12 kbit/s");
        assert_eq!(rate(0.0), "0 kbit/s");
    }

    #[test]
    fn a_column_is_the_same_width_whatever_is_in_it() {
        assert_eq!(column("4.51 Gbit/s").len(), column("940.0 Mbit/s").len());
        assert_eq!(column("0.184 ms").len(), 12);
    }

    #[test]
    fn bytes_per_second_is_the_same_measurement_in_the_other_base() {
        let counted = Counted {
            bytes: 1024 * 1024 * 100,
            elapsed: Duration::from_secs(1),
        };
        assert_eq!(bytes_per_second(&counted), "100 MiB/s");
        assert_eq!(bytes_per_second(&Counted::default()), "0 MiB/s");
    }
}
