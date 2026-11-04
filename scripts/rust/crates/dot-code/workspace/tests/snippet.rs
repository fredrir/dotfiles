#![forbid(unsafe_code)]

use dot_code_workspace::snippet::{Params, markdown};

fn render(path: &str, root: Option<&str>, text: &str) -> String {
    markdown(&Params {
        path: path.into(),
        root: root.map(Into::into),
        text: text.into(),
    })
}

#[test]
fn files_in_the_workspace_use_their_relative_path() {
    assert_eq!(
        render("/repo/src/main.rs", Some("/repo"), "fn main() {}\n"),
        "---\n**src/main.rs:**\n```rs\nfn main() {}\n```\n---\n\n"
    );
}

#[test]
fn files_outside_the_workspace_keep_their_absolute_path() {
    assert_eq!(
        render("/elsewhere/a.go", Some("/repo"), "package a\n"),
        "---\n**/elsewhere/a.go:**\n```go\npackage a\n```\n---\n\n"
    );
    assert_eq!(
        render("Untitled-1", None, "draft\n"),
        "---\n**Untitled-1:**\n```\ndraft\n```\n---\n\n"
    );
}

#[test]
fn the_language_is_the_last_extension_of_the_name() {
    assert_eq!(
        render("/repo/a.test.tsx", None, "").lines().nth(2),
        Some("```tsx")
    );
    assert_eq!(
        render("/repo/.zshrc", None, "").lines().nth(2),
        Some("```zshrc")
    );
    assert_eq!(
        render("/repo.d/Dockerfile", None, "").lines().nth(2),
        Some("```")
    );
}

#[test]
fn text_without_a_trailing_newline_still_closes_the_fence() {
    assert_eq!(
        render("/repo/a.txt", Some("/repo"), "last"),
        "---\n**a.txt:**\n```txt\nlast\n```\n---\n\n"
    );
    assert_eq!(
        render("/repo/a.txt", Some("/repo"), ""),
        "---\n**a.txt:**\n```txt\n```\n---\n\n"
    );
}

#[test]
fn the_fence_outgrows_backticks_in_the_text() {
    assert_eq!(
        render("/repo/README.md", Some("/repo"), "```sh\nls\n```\n"),
        "---\n**README.md:**\n````md\n```sh\nls\n```\n````\n---\n\n"
    );
}
