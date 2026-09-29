use super::*;
use testkit::tree;

const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn levels() -> Vec<String> {
    LEVELS.map(String::from).to_vec()
}

fn model(reasoning: bool, map: &[(&str, Option<&str>)]) -> Model {
    Model {
        reasoning,
        levels: (!map.is_empty()).then(|| {
            map.iter()
                .map(|(level, mapped)| (level.to_string(), mapped.map(String::from)))
                .collect()
        }),
        ..Model::default()
    }
}

#[test]
fn the_model_table_is_read_by_its_header() {
    let text = "provider       model            context  max-out  thinking  images\n\
                alibaba-cloud  qwen3.8-flash    1M       131.1K   yes       yes\n\
                deepseek       deepseek-v4-pro  1M       384K     no        no\n\n";
    assert_eq!(
        parse_list(text),
        [
            ("alibaba-cloud".into(), "qwen3.8-flash".into(), Some(true)),
            ("deepseek".into(), "deepseek-v4-pro".into(), Some(false)),
        ]
    );
    assert!(parse_list("no header here\n").is_empty());
}

#[test]
fn a_model_that_does_not_reason_only_turns_thinking_off() {
    assert_eq!(model(false, &[]).thinking_levels(&levels()), ["off"]);
}

#[test]
fn the_highest_levels_need_an_explicit_mapping() {
    assert_eq!(
        model(true, &[]).thinking_levels(&levels()),
        ["off", "minimal", "low", "medium", "high"]
    );
}

#[test]
fn a_mapping_to_null_removes_a_level() {
    let mapped = model(
        true,
        &[
            ("minimal", None),
            ("medium", None),
            ("low", Some("low")),
            ("max", Some("max")),
        ],
    );
    assert_eq!(
        mapped.thinking_levels(&levels()),
        ["off", "low", "high", "max"]
    );
}

#[test]
fn catalogs_supply_names_and_level_maps() {
    let root = tree(&[
        "ai/dist/providers/data/deepseek.json={\"openai-completions\":{\"flash\":{\"id\":\"flash\",\"provider\":\"deepseek\",\"name\":\"Flash\",\"reasoning\":true,\"thinkingLevelMap\":{\"minimal\":null,\"max\":\"max\"}}}}",
        "coding-agent/package.json={}",
        "agent/models.json={\"providers\":{\"local\":{\"models\":[{\"id\":\"llama\",\"name\":\"Llama\"}]}}}",
    ]);
    let pi = Pi {
        binary: None,
        package: Some(root.path().join("coding-agent")),
        agent_dir: root.path().join("agent"),
    };
    let catalog = catalog(&pi);
    let flash = catalog
        .get(&("deepseek".into(), "flash".into()))
        .expect("flash");
    assert_eq!(flash.name, "Flash");
    assert!(flash.reasoning);
    assert_eq!(
        flash
            .levels
            .as_ref()
            .and_then(|levels| levels.get("minimal")),
        Some(&None)
    );
    let llama = catalog
        .get(&("local".into(), "llama".into()))
        .expect("custom model");
    assert_eq!(llama.name, "Llama");
    assert!(!llama.reasoning);
}
