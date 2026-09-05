use std::fs;
use std::time::SystemTime;

use hostkit::ssh::{self, Resolved};

use super::model::{MasterInfo, TargetInfo};

pub fn resolve(target: &str, diagnostics: bool) -> TargetInfo {
    match ssh::resolve(target) {
        Ok(resolved) => {
            let mut info = describe(target, resolved);
            if diagnostics {
                info.master = master(target, info.master.control_path.take());
            }
            info
        }
        Err(error) => TargetInfo {
            input: target.to_string(),
            error: Some(error),
            ..TargetInfo::default()
        },
    }
}

fn describe(target: &str, resolved: Resolved) -> TargetInfo {
    TargetInfo {
        input: target.to_string(),
        hostname: resolved.hostname,
        route: Some(resolved.route),
        bound: resolved.bound,
        proxy: resolved.proxy,
        user: resolved.user,
        port: resolved.port,
        master: MasterInfo {
            control_path: resolved.control_path,
            ..MasterInfo::default()
        },
        error: None,
    }
}

fn master(target: &str, control_path: Option<String>) -> MasterInfo {
    let checked = ssh::control_master(target);
    let age = control_path
        .as_deref()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    MasterInfo {
        running: checked.alive,
        control_path,
        age,
        detail: checked.detail,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/info/ssh_info_tests.rs"]
mod tests;
