use std::fs;
use std::process::Command;
use std::time::SystemTime;

use hostkit::ssh;

use super::model::{MasterInfo, TargetInfo};

pub fn resolve(target: &str, diagnostics: bool) -> TargetInfo {
    match config(target) {
        Ok((text, mut info)) => {
            info.route = ssh::classify(&text);
            if diagnostics {
                info.master = master(target, info.master.control_path.take());
            }
            info
        }
        Err(error) => TargetInfo {
            input: target.to_string(),
            hostname: String::new(),
            route: None,
            bound: None,
            proxy: None,
            user: None,
            port: None,
            master: MasterInfo::default(),
            error: Some(error),
        },
    }
}

fn config(target: &str) -> Result<(String, TargetInfo), String> {
    let output = Command::new("ssh")
        .args(["-G", "--", target])
        .output()
        .map_err(|error| format!("ssh: {error}"))?;
    if !output.status.success() {
        return Err(reason(&output.stderr, "no config resolved"));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "ssh -G returned non-UTF-8 output".to_string())?;
    let mut info = TargetInfo {
        input: target.to_string(),
        hostname: String::new(),
        route: None,
        bound: None,
        proxy: None,
        user: None,
        port: None,
        master: MasterInfo::default(),
        error: None,
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => info.hostname = value.to_string(),
            "bindinterface" if value != "none" => info.bound = Some(value.to_string()),
            "bindaddress" if value != "none" && info.bound.is_none() => {
                info.bound = Some(value.to_string());
            }
            "proxycommand" if value != "none" => info.proxy = Some(value.to_string()),
            "user" => info.user = Some(value.to_string()),
            "port" => info.port = value.parse().ok(),
            "controlpath" if value != "none" => {
                info.master.control_path = Some(value.to_string());
            }
            _ => {}
        }
    }
    if info.hostname.is_empty() {
        return Err("ssh -G returned no hostname".into());
    }
    Ok((text, info))
}

fn master(target: &str, control_path: Option<String>) -> MasterInfo {
    let output = Command::new("ssh")
        .args(["-O", "check", "--", target])
        .output();
    let (running, detail) = match output {
        Ok(output) => {
            let detail = if output.stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };
            (
                output.status.success(),
                (!detail.is_empty()).then_some(detail),
            )
        }
        Err(error) => (false, Some(format!("ssh: {error}"))),
    };
    let age = control_path
        .as_deref()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    MasterInfo {
        running,
        control_path,
        age,
        detail,
    }
}

fn reason(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_reasons_win_over_the_fallback() {
        assert_eq!(reason(b"no host\n", "fallback"), "no host");
        assert_eq!(reason(b"", "fallback"), "fallback");
    }
}
