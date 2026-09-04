use std::env;
use std::hint::black_box;
use std::process::ExitCode;
use std::thread;
use std::time::Instant;

const DEFAULT_CPU_ITERATIONS: u64 = 800_000_000;
const DEFAULT_MEMORY_MIB: usize = 256;
const DEFAULT_MEMORY_PASSES: u32 = 128;

const USAGE: &str = "usage: bench-workloads <cpu|memory> [options]
       bench-workloads --list | --version

cpu options:
  --threads <n>      worker threads, 0 = all logical cores (default 1)
  --iterations <n>   iterations per thread
memory options:
  --op <read|write>  measured direction (default read)
  --mib <n>          buffer size in MiB
  --passes <n>       full passes over the buffer";

struct Measurement {
    workload: &'static str,
    unit: &'static str,
    value: f64,
    elapsed_s: f64,
    threads: usize,
    detail: Vec<(&'static str, u64)>,
}

impl Measurement {
    // Hand-rolled on purpose: the fields are numbers and fixed ASCII names,
    // and keeping the crate dependency-free keeps setup builds instant.
    fn to_json(&self) -> String {
        let detail = self
            .detail
            .iter()
            .map(|(key, value)| format!("\"{key}\":{value}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"workload\":\"{}\",\"unit\":\"{}\",\"value\":{:.3},\"elapsed_s\":{:.3},\"threads\":{},\"detail\":{{{detail}}}}}",
            self.workload, self.unit, self.value, self.elapsed_s, self.threads
        )
    }
}

fn xorshift_chain(seed: u64, iterations: u64) -> u64 {
    let mut state = seed | 1;
    let mut sum = 0u64;
    for _ in 0..iterations {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        sum = sum.wrapping_add(state.wrapping_mul(0x2545_F491_4F6C_DD1D));
    }
    sum
}

fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    thread::available_parallelism().map_or(1, |count| count.get())
}

fn cpu_workload(requested_threads: usize, iterations: u64) -> Measurement {
    let threads = resolve_threads(requested_threads);
    let started = Instant::now();
    if threads == 1 {
        black_box(xorshift_chain(0x9E37_79B9_7F4A_7C15, iterations));
    } else {
        let handles: Vec<_> = (0..threads)
            .map(|index| {
                let seed = 0x9E37_79B9_7F4A_7C15 ^ ((index as u64 + 1) << 32);
                thread::spawn(move || black_box(xorshift_chain(seed, iterations)))
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
    }
    let elapsed_s = started.elapsed().as_secs_f64();
    Measurement {
        workload: "cpu",
        unit: "Mops/s",
        value: (iterations as f64 * threads as f64) / elapsed_s / 1e6,
        elapsed_s,
        threads,
        detail: vec![("iterations", iterations)],
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MemoryOp {
    Read,
    Write,
}

fn memory_workload(op: MemoryOp, mib: usize, passes: u32) -> Measurement {
    let words = mib * 1024 * 1024 / 8;
    let mut buffer: Vec<u64> = vec![0u64; words];
    for (index, slot) in buffer.iter_mut().enumerate() {
        *slot = (index as u64) ^ 0xA5A5_A5A5_A5A5_A5A5;
    }
    let started = Instant::now();
    match op {
        MemoryOp::Write => {
            for pass in 0..passes {
                let base = u64::from(pass).wrapping_mul(0x0101_0101_0101_0101);
                for (index, slot) in buffer.iter_mut().enumerate() {
                    *slot = base.wrapping_add(index as u64);
                }
                black_box(&buffer);
            }
        }
        MemoryOp::Read => {
            // Eight independent accumulators, so the loop is limited by how
            // fast the core can pull cache lines, not by the latency of one
            // serial add chain.
            for _ in 0..passes {
                let mut sums = [0u64; 8];
                for chunk in buffer.chunks_exact(8) {
                    for (sum, value) in sums.iter_mut().zip(chunk) {
                        *sum = sum.wrapping_add(*value);
                    }
                }
                black_box(sums);
            }
        }
    }
    let elapsed_s = started.elapsed().as_secs_f64();
    let bytes = words as f64 * 8.0 * f64::from(passes);
    Measurement {
        workload: "memory",
        unit: "GiB/s",
        value: bytes / elapsed_s / f64::from(1u32 << 30),
        elapsed_s,
        threads: 1,
        detail: vec![("buffer_mib", mib as u64), ("passes", u64::from(passes))],
    }
}

struct Arguments {
    values: Vec<String>,
    position: usize,
}

impl Arguments {
    fn flag_value(&mut self, flag: &str) -> Result<Option<&str>, String> {
        if self.values.get(self.position).map(String::as_str) != Some(flag) {
            return Ok(None);
        }
        self.position += 1;
        match self.values.get(self.position) {
            Some(value) => {
                self.position += 1;
                Ok(Some(value))
            }
            None => Err(format!("{flag} needs a value")),
        }
    }

    fn finished(&self) -> Result<(), String> {
        match self.values.get(self.position) {
            Some(extra) => Err(format!("unexpected argument: {extra}")),
            None => Ok(()),
        }
    }
}

fn parse_number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} expects a number, got '{text}'"))
}

fn run_cpu(arguments: &mut Arguments) -> Result<Measurement, String> {
    let mut threads = 1usize;
    let mut iterations = DEFAULT_CPU_ITERATIONS;
    loop {
        if let Some(value) = arguments.flag_value("--threads")? {
            threads = parse_number("--threads", value)?;
        } else if let Some(value) = arguments.flag_value("--iterations")? {
            iterations = parse_number("--iterations", value)?;
        } else {
            arguments.finished()?;
            return Ok(cpu_workload(threads, iterations));
        }
    }
}

fn run_memory(arguments: &mut Arguments) -> Result<Measurement, String> {
    let mut op = MemoryOp::Read;
    let mut mib = DEFAULT_MEMORY_MIB;
    let mut passes = DEFAULT_MEMORY_PASSES;
    loop {
        if let Some(value) = arguments.flag_value("--op")? {
            op = match value {
                "read" => MemoryOp::Read,
                "write" => MemoryOp::Write,
                other => return Err(format!("--op expects read or write, got '{other}'")),
            };
        } else if let Some(value) = arguments.flag_value("--mib")? {
            mib = parse_number("--mib", value)?;
            if mib == 0 {
                return Err("--mib must be at least 1".into());
            }
        } else if let Some(value) = arguments.flag_value("--passes")? {
            passes = parse_number("--passes", value)?;
        } else {
            arguments.finished()?;
            return Ok(memory_workload(op, mib, passes));
        }
    }
}

fn dispatch(values: Vec<String>) -> Result<Option<Measurement>, String> {
    let command = values.first().cloned().unwrap_or_default();
    let mut arguments = Arguments {
        values,
        position: 1,
    };
    match command.as_str() {
        "--version" => {
            println!("bench-workloads {}", env!("CARGO_PKG_VERSION"));
            Ok(None)
        }
        "--list" => {
            println!("cpu\nmemory");
            Ok(None)
        }
        "cpu" => run_cpu(&mut arguments).map(Some),
        "memory" => run_memory(&mut arguments).map(Some),
        "" => Err("missing workload".into()),
        other => Err(format!("unknown workload: {other}")),
    }
}

fn main() -> ExitCode {
    match dispatch(env::args().skip(1).collect()) {
        Ok(Some(measurement)) => {
            println!("{}", measurement.to_json());
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("bench-workloads: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;
