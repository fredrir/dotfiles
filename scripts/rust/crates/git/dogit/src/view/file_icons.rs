use ui_theme::Color;

use super::{Style, color_depth};

const BLUE: Color = Color::Rgb(0x51, 0x9a, 0xba);
const GRAY: Color = Color::Rgb(0x6d, 0x80, 0x86);
const GIT_GRAY: Color = Color::Rgb(0x41, 0x53, 0x5b);
const GREEN: Color = Color::Rgb(0x8d, 0xc1, 0x49);
const ORANGE: Color = Color::Rgb(0xe3, 0x79, 0x33);
const PINK: Color = Color::Rgb(0xf5, 0x53, 0x85);
const PURPLE: Color = Color::Rgb(0xa0, 0x74, 0xc4);
const RED: Color = Color::Rgb(0xcc, 0x3e, 0x44);
const YELLOW: Color = Color::Rgb(0xcb, 0xcb, 0x41);

pub(super) fn appearance(path: &str, appearance: Style) -> (&'static str, Style) {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let (icon, color) = match name.as_str() {
        "dockerfile" | "containerfile" => ("\u{e7b0}", BLUE),
        "compose.yaml" | "compose.yml" => ("\u{e7b0}", PINK),
        ".gitignore" | ".gitattributes" | ".gitmodules" => ("\u{e702}", GIT_GRAY),
        "cargo.lock" | ".env" => ("\u{e615}", GRAY),
        "package-lock.json" => ("\u{e615}", YELLOW),
        "yarn.lock" => ("\u{e615}", BLUE),
        "pnpm-lock.yaml" => ("\u{e615}", PURPLE),
        "makefile" | "justfile" => ("\u{e615}", ORANGE),
        _ => match name.rsplit_once('.').map(|(_, extension)| extension) {
            Some("ts" | "mts" | "cts") => ("\u{e628}", BLUE),
            Some("tsx" | "jsx") => ("\u{e625}", BLUE),
            Some("js" | "mjs" | "cjs") => ("\u{e781}", YELLOW),
            Some("lua") => ("\u{e620}", BLUE),
            Some("rs") => ("\u{e7a8}", GRAY),
            Some("py" | "pyi" | "pyw") => ("\u{e73c}", BLUE),
            Some("go") => ("\u{e724}", BLUE),
            Some("c") => ("\u{e649}", BLUE),
            Some("h") => ("\u{e649}", PURPLE),
            Some("cpp" | "cc" | "cxx") => ("\u{e7a3}", BLUE),
            Some("hpp") => ("\u{e7a3}", PURPLE),
            Some("java") => ("\u{e738}", RED),
            Some("class") => ("\u{e738}", BLUE),
            Some("swift") => ("\u{e755}", ORANGE),
            Some("html" | "htm") => ("\u{e736}", ORANGE),
            Some("css" | "less") => ("\u{e749}", BLUE),
            Some("scss" | "sass") => ("\u{e749}", PINK),
            Some("vue") => ("\u{e6a0}", GREEN),
            Some("svelte") => ("\u{e697}", RED),
            Some("json" | "jsonc" | "json5") => ("\u{e60b}", YELLOW),
            Some("toml" | "ini" | "conf" | "cfg") => ("\u{e615}", GRAY),
            Some("yaml" | "yml") => ("\u{e615}", PURPLE),
            Some("md" | "mdx" | "markdown") => ("\u{e609}", BLUE),
            Some("sh" | "bash" | "zsh" | "fish") => ("\u{e691}", GREEN),
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") => ("\u{e60d}", PURPLE),
            Some("ico") => ("\u{e60d}", YELLOW),
            _ => return ("\u{e64e}", appearance),
        },
    };
    (icon, appearance.fg(color.at_depth(color_depth()).ratatui()))
}
