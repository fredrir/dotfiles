use std::ffi::OsString;

pub fn runtime_dir(current: Option<OsString>, uid: u32) -> OsString {
    current
        .filter(|directory| !directory.is_empty())
        .unwrap_or_else(|| OsString::from(format!("/run/user/{uid}")))
}

#[cfg(test)]
#[path = "../tests/unit/env_tests.rs"]
mod tests;
