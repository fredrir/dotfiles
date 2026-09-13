use std::fs;
use std::path::Path;

pub fn repository() -> tempfile::TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    for path in [
        "theme/profiles",
        "theme/maps",
        "theme/roles.toml",
        "theme/fonts.toml",
        "shared/fastfetch/config.jsonc",
        "linux/arch/fastfetch/config.jsonc",
        "linux/arch/fastfetch/arch.txt",
        "linux/ubuntu/fastfetch/config.jsonc",
        "linux/ubuntu/fastfetch/ubuntu.txt",
        "macos/fastfetch/config.jsonc",
        "macos/fastfetch/apple.txt",
        "shared/starship/starship.toml",
        "shared/obsidian/themes/Fredrir/theme.css",
        "shared/nvim/lua/ui/theme.lua",
        "shared/nvim/plugins.lua",
        "linux/common/gtk/gtk-3.0/colors.css",
        "linux/common/gtk/gtk-3.0/settings.ini",
        "linux/common/gtk/gtk-4.0/colors.css",
        "linux/common/gtk/gtk-4.0/settings.ini",
        "linux/common/quicklaunch/config.toml",
        "linux/kde/panel-colorizer/presets",
        "linux/kde/plasma/kdeglobals",
        "linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc",
    ] {
        copy(&source.join(path), &directory.path().join(path));
    }
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(directory.path().join("config/targets.dotfile"), "").unwrap();
    fs::write(
        directory.path().join("config/profiles.dotfile"),
        "shared {\n  theme = mocha\n}\n",
    )
    .unwrap();
    directory
}

fn copy(source: &Path, destination: &Path) {
    if source.is_dir() {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            copy(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(source, destination)
            .unwrap_or_else(|error| panic!("copy theme input {}: {error}", source.display()));
    }
}
