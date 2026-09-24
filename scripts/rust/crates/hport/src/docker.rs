use std::fs;
use std::net::IpAddr;

use crate::listener::Listener;

const PROXY: &str = "docker-proxy";

// docker-proxy runs as root, so only its world-readable argv names the port
pub fn proxies() -> Vec<Listener> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let comm = fs::read_to_string(entry.path().join("comm")).ok()?;
            if comm.trim_end() != PROXY {
                return None;
            }
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let args = cmdline
                .split(|byte| *byte == 0)
                .filter_map(|arg| std::str::from_utf8(arg).ok())
                .collect::<Vec<_>>();
            let (address, port) = published(&args)?;
            Some(Listener {
                port,
                address,
                process: PROXY.to_string(),
                pid,
            })
        })
        .collect()
}

pub fn published(args: &[&str]) -> Option<(IpAddr, u16)> {
    let value = |flag: &str| {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1])
    };
    if value("-proto")? != "tcp" {
        return None;
    }
    Some((
        value("-host-ip")?.parse().ok()?,
        value("-host-port")?.parse().ok()?,
    ))
}

#[cfg(test)]
#[path = "../tests/unit/docker_tests.rs"]
mod tests;
