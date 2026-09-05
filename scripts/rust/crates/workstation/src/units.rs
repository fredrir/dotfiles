pub fn bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    match value {
        _ if value >= KIB * KIB * KIB => format!("{:.2} GiB", value / (KIB * KIB * KIB)),
        _ if value >= KIB * KIB => format!("{:.1} MiB", value / (KIB * KIB)),
        _ if value >= KIB => format!("{:.0} KiB", value / KIB),
        _ => format!("{bytes} B"),
    }
}

#[cfg(test)]
#[path = "../tests/unit/units_tests.rs"]
mod tests;
