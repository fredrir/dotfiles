use std::fs;
use std::path::Path;

use super::*;
use testkit::tree;

const MODELS: &str = "provider  model   context  max-out  thinking  images\n\
                      deepseek  flash   1M       384K     yes       yes\n\
                      deepseek  pro     1M       384K     no        no\n\
                      openai    gpt     1M       128K     yes       yes\n";

// A pi install: the package with its catalog and tools, and a `pi` that answers from the fixtures.
fn install() -> testkit::TempDir {
    let fixtures = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let root = tree(&[
        "pi/coding-agent/package.json={\"name\":\"@earendil-works/pi-coding-agent\"}",
        "pi/coding-agent/dist/core/tools/index.js=export const allToolNames = new Set([\"read\", \"bash\", \"edit\"]);",
        "pi/coding-agent/dist/modes/interactive/theme/dark.json={\"name\":\"dark\"}",
        "pi/ai/dist/providers/data/deepseek.json={\"api\":{\"flash\":{\"id\":\"flash\",\"provider\":\"deepseek\",\"name\":\"Flash\",\"reasoning\":true,\"thinkingLevelMap\":{\"minimal\":null,\"max\":\"max\"}}}}",
        "home/.pi/agent/settings.json={\"defaultProvider\":\"deepseek\",\"defaultModel\":\"flash\",\"packages\":[\"npm:pi-web\"]}",
        "work/",
        "bin/",
    ]);
    let script = format!(
        "#!/bin/sh\nfor a; do [ \"$a\" = --list-models ] && {{ printf '%s' '{MODELS}'; exit 0; }}; done\n\
         case \"$1\" in\n  uninstall) cat {fixtures}/pi-remove.txt ;;\n  \
         install|remove|update|list|config|auth) cat {fixtures}/pi-$1.txt ;;\n  *) cat {fixtures}/pi.txt ;;\nesac\n"
    );
    let cli = root.path().join("pi/coding-agent/dist/cli.js");
    testkit::executable(&cli, &script);
    std::os::unix::fs::symlink(&cli, root.path().join("bin/pi")).unwrap();
    root
}

fn completed(root: &Path, line: &str) -> Reply {
    let path = root.join("bin").to_string_lossy().to_string();
    let ctx = Context::testing(&root.join("home"), &root.join("work"), &[("PATH", &path)]);
    let words: Vec<String> = line.split(' ').map(String::from).collect();
    let current = words.len();
    complete(&ctx, &Line::new(words, current, None))
}

#[test]
fn the_first_word_offers_pi_s_commands() {
    let root = install();
    let reply = completed(root.path(), "pi ");
    assert_eq!(
        reply.values(),
        [
            "install",
            "remove",
            "uninstall",
            "update",
            "list",
            "config",
            "auth"
        ]
    );
}

#[test]
fn flags_come_from_the_help_including_extension_flags() {
    let root = install();
    let values = completed(root.path(), "pi --").values().join(" ");
    for expected in ["-nt", "--no-tools", "--model=", "--mcp-config="] {
        assert!(
            values.split(' ').any(|value| value == expected),
            "{expected} in {values}"
        );
    }
}

#[test]
fn models_are_offered_with_their_names_and_the_default() {
    let root = install();
    let reply = completed(root.path(), "pi --model ");
    assert_eq!(
        reply.values(),
        ["flash", "pro", "gpt", "deepseek/", "openai/"]
    );
    assert!(
        reply
            .plain()
            .contains("item\tflash\tflash  deepseek  Flash  default"),
        "{}",
        reply.plain()
    );
}

#[test]
fn a_given_provider_narrows_the_models() {
    let root = install();
    assert_eq!(
        completed(root.path(), "pi --provider openai --model ").values(),
        ["gpt"]
    );
    let reply = completed(root.path(), "pi --model deepseek/");
    assert!(reply.plain().starts_with("skip\tdeepseek/\n"));
    assert_eq!(reply.values(), ["flash", "pro"]);
}

#[test]
fn thinking_levels_follow_the_model() {
    let root = install();
    assert_eq!(
        completed(root.path(), "pi --model flash:").values(),
        ["off", "low", "medium", "high", "max"]
    );
    assert_eq!(
        completed(root.path(), "pi --model deepseek/pro:").values(),
        ["off"]
    );
    assert_eq!(
        completed(root.path(), "pi --model=gpt:").values(),
        ["off", "minimal", "low", "medium", "high"]
    );
}

#[test]
fn providers_count_their_models() {
    let root = install();
    let rendered = completed(root.path(), "pi --provider ").plain();
    assert!(
        rendered.contains("item\tdeepseek\tdeepseek  2 models  default"),
        "{rendered}"
    );
    assert!(
        rendered.contains("item\topenai\topenai    1 model\n"),
        "{rendered}"
    );
}

#[test]
fn listed_values_complete_one_element_at_a_time() {
    let root = install();
    let reply = completed(root.path(), "pi --tools read,");
    assert!(reply.plain().starts_with("skip\tread,\n"));
    assert_eq!(reply.values(), ["bash", "edit"]);
    let models = completed(root.path(), "pi --models flash,");
    assert!(models.values().contains(&"gpt"));
}

#[test]
fn values_listed_in_the_help_are_offered() {
    let root = install();
    assert_eq!(
        completed(root.path(), "pi --mode ").values(),
        ["text", "json", "rpc"]
    );
    assert_eq!(
        completed(root.path(), "pi --tui-mode=").values(),
        ["regular", "fullscreen"]
    );
}

#[test]
fn subcommands_complete_their_own_arguments() {
    let root = install();
    assert_eq!(
        completed(root.path(), "pi remove ").values(),
        ["npm:pi-web"]
    );
    assert_eq!(
        completed(root.path(), "pi update ").values(),
        ["self", "pi", "npm:pi-web"]
    );
    assert_eq!(
        completed(root.path(), "pi auth ").values(),
        ["print-api-key", "print-bearer-token", "check"]
    );
    assert!(
        completed(root.path(), "pi auth check --")
            .values()
            .contains(&"--provider=")
    );
    assert!(
        completed(root.path(), "pi remove npm:pi-web ")
            .values()
            .is_empty()
    );
}

#[test]
fn install_offers_source_types_before_the_gallery_is_known() {
    let root = install();
    let reply = completed(root.path(), "pi install ");
    assert_eq!(reply.values(), ["npm:", "git:", "https://", "ssh://"]);
    assert_eq!(completed(root.path(), "pi install ./").plain(), "files\n");
}

#[test]
fn file_references_complete_as_files() {
    let root = install();
    assert_eq!(completed(root.path(), "pi @").plain(), "skip\t@\nfiles\n");
    assert_eq!(completed(root.path(), "pi --export ").plain(), "files\n");
    assert_eq!(
        completed(root.path(), "pi --session-dir ").plain(),
        "dirs\n"
    );
}

#[test]
fn sessions_for_this_directory_are_offered_newest_first() {
    let root = install();
    let dir = sessions::default_directory(
        &root.path().join("home/.pi/agent"),
        &root.path().join("work"),
    );
    fs::create_dir_all(&dir).unwrap();
    let header = |id: &str| {
        format!(
            "{{\"type\":\"session\",\"id\":\"{id}\",\"cwd\":\"{}\"}}\n",
            root.path().join("work").display()
        )
    };
    fs::write(dir.join("old.jsonl"), header("older")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let named = format!(
        "{}{}\n",
        header("newer"),
        r#"{"type":"session_info","name":"Refactor"}"#
    );
    fs::write(dir.join("new.jsonl"), named).unwrap();
    let reply = completed(root.path(), "pi --session ");
    assert_eq!(reply.values(), ["newer", "older"]);
    assert!(
        reply.plain().contains("just now  Refactor"),
        "{}",
        reply.plain()
    );
}

#[test]
fn themes_can_be_paired_for_dark_and_light() {
    let root = install();
    let reply = completed(root.path(), "pi --use-theme dark/");
    assert!(reply.plain().starts_with("skip\tdark/\n"));
    assert_eq!(reply.values(), ["dark"]);
}
