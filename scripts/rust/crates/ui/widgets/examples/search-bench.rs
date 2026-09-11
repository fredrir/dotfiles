use std::hint::black_box;
use std::time::{Duration, Instant};
use ui_widgets::{MatchMode, SearchIndex, SearchText};

const QUERIES: [&str; 4] = [
    "project-42",
    "host-3 agent-2",
    "session-991",
    "project-7 host-11",
];

fn main() {
    let rows = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100_000)
        .max(1);
    let labels: Vec<_> = (0..rows).map(|index| format!("project-{} host-{} agent-{} session-{index:06} working directory /workspace/repository-{}", index % 997, index % 17, index % 11, index % 257)).collect();
    let started = Instant::now();
    let index = SearchIndex::new(&labels);
    let build = started.elapsed();
    let bytes = (0..rows)
        .map(|row| index.entry(row).unwrap().normalized().len())
        .sum::<usize>()
        + rows * std::mem::size_of::<SearchText>();
    let expected = queries(&index);
    let mut cached = Vec::new();
    let mut rebuilt = Vec::new();
    for sample in 0..7 {
        // Alternate order to reduce systematic warm-cache bias.
        let mut run = |cached_first| {
            if cached_first {
                let (duration, checksum) = timed(|| queries(&index));
                assert_eq!(checksum, expected);
                cached.push(duration);
            } else {
                let (duration, checksum) = timed(|| {
                    QUERIES.iter().fold(0u64, |sum, query| {
                        sum.wrapping_add(checksum(
                            SearchIndex::new(&labels).search(query, MatchMode::Fuzzy),
                        ))
                    })
                });
                assert_eq!(checksum, expected);
                rebuilt.push(duration);
            }
        };
        run(sample % 2 == 0);
        run(sample % 2 != 0);
    }
    cached.sort_unstable();
    rebuilt.sort_unstable();
    let cached = cached[cached.len() / 2];
    let rebuilt = rebuilt[rebuilt.len() / 2];
    println!(
        "rows={rows} queries={} samples=7 checksum={expected}",
        QUERIES.len()
    );
    println!(
        "cache_build_ms={:.3} cache_payload_bytes={bytes}",
        build.as_secs_f64() * 1000.0
    );
    println!(
        "cached_median_ms={:.3} rebuilt_median_ms={:.3} ratio={:.2}x",
        cached.as_secs_f64() * 1000.0,
        rebuilt.as_secs_f64() * 1000.0,
        rebuilt.as_secs_f64() / cached.as_secs_f64()
    );
}

fn queries(index: &SearchIndex) -> u64 {
    QUERIES.iter().fold(0u64, |sum, query| {
        sum.wrapping_add(checksum(index.search(black_box(query), MatchMode::Fuzzy)))
    })
}

fn checksum(rows: Vec<usize>) -> u64 {
    rows.into_iter()
        .enumerate()
        .fold(0u64, |sum, (rank, index)| {
            sum.wrapping_add((rank as u64 + 1).wrapping_mul(index as u64 + 1))
        })
}

fn timed(run: impl FnOnce() -> u64) -> (Duration, u64) {
    let started = Instant::now();
    let result = black_box(run());
    (started.elapsed(), result)
}
