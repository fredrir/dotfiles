use super::*;
use clap::Parser;

const LIST: &str = "0: 1002:13C0-1043:8877-0000:0c:00.0 (Radeon Graphics) [Integrated]\n1: 10DE:2C05-1458:4181-0000:01:00.0 (GeForce RTX 5070 Ti) [Dedicated]\n";
const LIMIT: &str = "Current power limit: 350W (Configurable Range: 250W to 350W)\n";

fn trial(cap: u32, values: &[(&str, f64)]) -> Trial {
    Trial {
        cap_w: cap,
        run_id: Some(format!("run-{cap}")),
        grade: Some("clean".into()),
        medians: values
            .iter()
            .map(|(key, value)| (key.to_string(), *value))
            .collect(),
        error: None,
    }
}

#[test]
fn list_output_selects_the_nvidia_gpu_over_the_integrated_one() {
    let gpus = parse_list(LIST);
    assert_eq!(gpus.len(), 2);
    assert_eq!(gpus[0].kind, "Integrated");
    let chosen = choose_gpu(&gpus).unwrap();
    assert_eq!(chosen.id, "10DE:2C05-1458:4181-0000:01:00.0");
    assert_eq!(chosen.name, "GeForce RTX 5070 Ti");
    assert!(parse_list("garbage\n\n").is_empty());
    assert!(choose_gpu(&[gpus[0].clone()]).is_err());
    let only_dedicated = Gpu {
        id: "1002:744C-1002:0E3B-0000:03:00.0".into(),
        name: "Navi 31".into(),
        kind: "Dedicated".into(),
    };
    assert_eq!(
        choose_gpu(&[gpus[0].clone(), only_dedicated.clone()]).unwrap(),
        &only_dedicated
    );
}

#[test]
fn power_limit_output_yields_current_and_range() {
    let limit = parse_limit(LIMIT).unwrap();
    assert_eq!(
        limit,
        Limit {
            current: 350.0,
            min: 250.0,
            max: 350.0
        }
    );
    assert!(parse_limit("Error: No cap reported by the GPU\n").is_err());
    assert!(parse_limit("Current power limit: 300W\n").is_err());
    assert!(parse_limit("Current power limit: 400W (Configurable Range: 250W to 350W)\n").is_err());
}

#[test]
fn caps_are_checked_against_the_range_and_deduplicated() {
    let limit = parse_limit(LIMIT).unwrap();
    assert_eq!(
        validate_caps(&[300, 250, 300, 350], &limit).unwrap(),
        vec![300, 250, 350]
    );
    assert!(validate_caps(&[], &limit).is_err());
    assert!(validate_caps(&[249], &limit).is_err());
    assert!(validate_caps(&[351], &limit).is_err());
}

#[test]
fn efficiency_needs_positive_power_and_caps_format_without_noise() {
    assert_eq!(per_watt(Some(300.0), Some(150.0)), Some(2.0));
    assert_eq!(per_watt(Some(300.0), Some(0.0)), None);
    assert_eq!(per_watt(None, Some(150.0)), None);
    assert_eq!(per_watt(Some(f64::NAN), Some(150.0)), None);
    assert_eq!(format_cap(350.0), "350");
    assert_eq!(format_cap(312.5), "312.5");
}

#[test]
fn table_rows_include_fans_only_when_measured() {
    let trials = vec![
        trial(
            300,
            &[
                ("ai.prompt_tps", 800.0),
                ("ai.generate_tps", 120.0),
                ("ai.gpu_w", 240.0),
            ],
        ),
        Trial {
            cap_w: 250,
            run_id: None,
            grade: None,
            medians: BTreeMap::new(),
            error: Some("no benchmark produced a result".into()),
        },
    ];
    let (headers, table) = rows(&trials);
    assert_eq!(
        headers,
        vec![
            "cap W",
            "prompt t/s",
            "generate t/s",
            "gpu W",
            "t/s per W",
            "run"
        ]
    );
    assert_eq!(table[0][0], "300");
    assert_eq!(table[0][4], "0.50");
    assert_eq!(table[0][5], "run-300");
    assert!(table[1][5].starts_with("failed:"));
    let with_fans = vec![trial(300, &[("idle.fan_rpm", 2100.0)])];
    let (headers, table) = rows(&with_fans);
    assert!(headers.contains(&"fan rpm"));
    assert_eq!(table[0][5], "2 100");
}

#[test]
fn medians_come_from_every_measured_metric() {
    let run = record::Run {
        metrics: vec![
            record::Metric {
                key: "ai.generate_tps".into(),
                samples: vec![100.0, 120.0, 110.0],
                ..record::Metric::default()
            },
            record::Metric {
                key: "idle.fan_rpm".into(),
                samples: vec![],
                ..record::Metric::default()
            },
        ],
        ..record::Run::default()
    };
    let values = medians(&run);
    assert_eq!(values.get("ai.generate_tps"), Some(&110.0));
    assert!(!values.contains_key("idle.fan_rpm"));
}

#[test]
fn sweep_arguments_parse_caps_families_and_settle() {
    let cli = crate::cli::Cli::try_parse_from([
        "hwtune", "gpu", "sweep", "--caps", "250,300", "--only", "ai,idle", "--settle", "5",
    ])
    .unwrap();
    let Some(crate::cli::Command::Gpu {
        command: Command::Sweep(options),
    }) = cli.command
    else {
        panic!("gpu sweep expected")
    };
    assert_eq!(options.caps, vec![250, 300]);
    assert_eq!(options.only, vec!["ai", "idle"]);
    assert_eq!(options.settle, 5);
    assert!(!options.json);
    assert!(crate::cli::Cli::try_parse_from(["hwtune", "gpu", "sweep"]).is_err());
    assert!(crate::cli::Cli::try_parse_from(["hwtune", "gpu", "sweep", "--caps", "0"]).is_err());
    assert!(families(&["ai".into(), "nope".into()]).is_err());
    assert_eq!(
        families(&["ai".into(), "ai".into(), "idle".into()]).unwrap(),
        vec!["ai", "idle"]
    );
}
