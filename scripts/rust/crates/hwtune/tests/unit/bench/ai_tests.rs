use super::*;

const TEXT: &str = "# bench settings\nai {\n  model   = ~/models/phi/Phi-4-mini-instruct-Q6_K.gguf\n  predict = 64\n}\n";

#[test]
fn settings_expand_home_and_default_predict() {
    let home = Path::new("/home/fixture");
    let parsed = parse_settings(TEXT, Some(home)).unwrap().unwrap();
    assert_eq!(
        parsed.model,
        Path::new("/home/fixture/models/phi/Phi-4-mini-instruct-Q6_K.gguf")
    );
    assert_eq!(parsed.predict, 64);
    let parsed = parse_settings("ai {\n  model = /models/a.gguf\n}\n", None)
        .unwrap()
        .unwrap();
    assert_eq!(parsed.model, Path::new("/models/a.gguf"));
    assert_eq!(parsed.predict, DEFAULT_PREDICT);
    assert_eq!(
        expand_home("~/x.gguf", None),
        Path::new("~/x.gguf"),
        "no home keeps the literal path"
    );
}

#[test]
fn settings_without_a_model_or_file_are_unavailable_and_bad_keys_fail() {
    assert_eq!(
        parse_settings("ai {\n  predict = 8\n}\n", None).unwrap(),
        None
    );
    assert_eq!(
        parse_settings("other {\n  model = /a\n}\n", None).unwrap(),
        None
    );
    assert!(parse_settings("ai {\n  model = /a\n  predict = 0\n}\n", None).is_err());
    assert!(parse_settings("ai {\n  model = /a\n  threads = 4\n}\n", None).is_err());
    assert!(parse_settings("ai {\n  model\n}\n", None).is_err());
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        load_settings(&temp.path().join("missing.bench.dotfile"), None).unwrap(),
        None
    );
    let file = temp.path().join("host.bench.dotfile");
    fs::write(&file, TEXT).unwrap();
    assert_eq!(
        load_settings(&file, Some(temp.path()))
            .unwrap()
            .unwrap()
            .model,
        temp.path().join("models/phi/Phi-4-mini-instruct-Q6_K.gguf")
    );
}

#[test]
fn settings_path_follows_the_host() {
    let paths = Paths {
        root: PathBuf::from("/repo"),
        host: "archie".into(),
    };
    assert_eq!(
        settings_path(&paths),
        Path::new("/repo/config/hwtune/archie.bench.dotfile")
    );
}

#[test]
fn summary_line_is_parsed_from_noisy_output() {
    let text = "load: ok\n> prompt\nanswer text\n\n[ Prompt: 798.2 t/s | Generation: 454.9 t/s ]\n\nExiting...\n";
    assert_eq!(parse_summary(text).unwrap(), (798.2, 454.9));
    let repeated =
        "[ Prompt: 1.0 t/s | Generation: 2.0 t/s ]\n[ Prompt: 3.5 t/s | Generation: 4.25 t/s ]";
    assert_eq!(parse_summary(repeated).unwrap(), (3.5, 4.25));
    assert!(parse_summary("no summary here").is_err());
    assert!(parse_summary("[ Prompt: 0 t/s | Generation: 4 t/s ]").is_err());
}

#[test]
fn version_is_the_build_number() {
    assert_eq!(
        parse_version("version: 0.2.0-dev (build 10586, commit b21e4de74)\nbuilt with GNU"),
        "10586"
    );
    assert_eq!(parse_version("unknown"), "");
}

#[test]
fn arguments_pin_a_deterministic_single_turn() {
    let args = arguments(&Settings {
        model: PathBuf::from("/m.gguf"),
        predict: 32,
    });
    assert_eq!(args[..2], ["-m", "/m.gguf"]);
    assert!(args.contains(&"-st".to_string()));
    assert!(args.contains(&"--perf".to_string()));
    assert_eq!(args[args.iter().position(|a| a == "-n").unwrap() + 1], "32");
    assert!(PROMPT.split_whitespace().count() > 250);
}

#[test]
fn gpu_power_averages_only_the_active_phase() {
    assert_eq!(active_mean(&[]), None);
    assert_eq!(active_mean(&[100.0]), Some(100.0));
    assert_eq!(active_mean(&[48.0, 120.0, 114.0, 126.0]), Some(120.0));
    assert_eq!(active_mean(&[50.0, 50.0, 50.0]), Some(50.0));
}
