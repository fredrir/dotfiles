// Nerd Font file-type glyphs; filenames remain readable independently of icons.
pub(super) fn for_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | "containerfile" | "compose.yaml" | "compose.yml" => "\u{e7b0}",
        ".gitignore" | ".gitattributes" | ".gitmodules" => "\u{e702}",
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" => "\u{e615}",
        "makefile" | "justfile" | ".env" => "\u{e615}",
        _ => match name.rsplit_once('.').map(|(_, extension)| extension) {
            Some("ts" | "mts" | "cts") => "\u{e628}",
            Some("tsx" | "jsx") => "\u{e625}",
            Some("js" | "mjs" | "cjs") => "\u{e781}",
            Some("lua") => "\u{e620}",
            Some("rs") => "\u{e7a8}",
            Some("py" | "pyi" | "pyw") => "\u{e73c}",
            Some("go") => "\u{e724}",
            Some("c" | "h") => "\u{e649}",
            Some("cpp" | "cc" | "cxx" | "hpp") => "\u{e7a3}",
            Some("java" | "class") => "\u{e738}",
            Some("swift") => "\u{e755}",
            Some("html" | "htm") => "\u{e736}",
            Some("css" | "scss" | "sass" | "less") => "\u{e749}",
            Some("vue") => "\u{e6a0}",
            Some("svelte") => "\u{e697}",
            Some("json" | "jsonc" | "json5") => "\u{e60b}",
            Some("toml" | "yaml" | "yml" | "ini" | "conf" | "cfg") => "\u{e615}",
            Some("md" | "mdx" | "markdown") => "\u{e609}",
            Some("sh" | "bash" | "zsh" | "fish") => "\u{e691}",
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico") => "\u{e60d}",
            _ => "\u{e64e}",
        },
    }
}
