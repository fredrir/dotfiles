use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::context::Context;

pub fn command_path(context: &Context, name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return executable(&path).then_some(path);
    }
    std::env::split_paths(&context.env("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn linux_platform(text: &str) -> &str {
    let values = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key, value.trim_matches(['\'', '"'])))
        .collect::<BTreeMap<_, _>>();
    match values.get("ID").copied().unwrap_or_default() {
        "arch" => "arch-linux",
        "ubuntu" => "ubuntu",
        _ => {
            let likes = values.get("ID_LIKE").copied().unwrap_or_default();
            if likes.split_whitespace().any(|name| name == "arch") {
                "arch-linux"
            } else if likes.split_whitespace().any(|name| name == "ubuntu") {
                "ubuntu"
            } else {
                ""
            }
        }
    }
}

pub fn platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".into(),
        "linux" => {
            linux_platform(&fs::read_to_string("/etc/os-release").unwrap_or_default()).into()
        }
        _ => String::new(),
    }
}

pub fn relevant(context: &Context) -> Result<Vec<String>, String> {
    let desktop = [
        (
            "linux/kde",
            ["plasmashell", "startplasma-wayland", "startplasma-x11"]
                .iter()
                .any(|name| command_path(context, name).is_some()),
        ),
        (
            "linux/hyprland",
            ["Hyprland", "hyprctl"]
                .iter()
                .any(|name| command_path(context, name).is_some()),
        ),
    ];
    filter(context, &platform(), &desktop)
}

pub fn filter(
    context: &Context,
    platform: &str,
    desktops: &[(&str, bool)],
) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    if platform.is_empty() {
        return Ok(found);
    }
    for profile in context.profiles()? {
        if (platform == "macos" && profile != "macos")
            || profile.split('/').next() != Some(platform)
        {
            continue;
        }
        let groups = super::read_manifest(&context.manifest(&profile))?;
        if desktops
            .iter()
            .all(|(group, installed)| *installed || !groups.iter().any(|entry| entry == group))
        {
            found.push(profile);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_distribution_and_derivative_detection() {
        for (input, expected) in [
            ("ID=arch\n", "arch-linux"),
            ("ID=ubuntu\n", "ubuntu"),
            ("ID=cachyos\nID_LIKE=arch\n", "arch-linux"),
            ("ID=neon\nID_LIKE=\"ubuntu debian\"\n", "ubuntu"),
            ("ID=gentoo\n", ""),
        ] {
            assert_eq!(linux_platform(input), expected);
        }
    }
}
