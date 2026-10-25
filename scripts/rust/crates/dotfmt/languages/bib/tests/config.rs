#![forbid(unsafe_code)]

use std::path::PathBuf;

use dotfmt_bib::config::Config;
use dotfmt_core::config::{Setting, Settings};

fn settings(values: &[(&str, &str)]) -> Settings {
    values
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: PathBuf::from("dotfmt.dotfile"),
                    line: 1,
                    global: false,
                },
            )
        })
        .collect()
}

#[test]
fn wrap_min_accepts_the_documented_range() {
    let config = Config::from_settings(&settings(&[("wrap_min", "1")])).unwrap();
    assert_eq!(config.wrap_min(), 1);
    let config = Config::from_settings(&settings(&[("wrap_min", "10000")])).unwrap();
    assert_eq!(config.wrap_min(), 10000);
}

#[test]
fn wrap_min_rejects_zero_and_oversized_values() {
    for value in ["0", "10001"] {
        let error = Config::from_settings(&settings(&[("wrap_min", value)])).unwrap_err();
        assert!(error.to_string().contains("wrap_min"), "{value}: {error}");
        assert!(
            error.to_string().contains("between 1 and 10000"),
            "{value}: {error}"
        );
    }
}

#[test]
fn wrap_min_defaults_to_ten_columns_below_the_width() {
    let config = Config {
        width: 80,
        ..Config::default()
    };
    assert_eq!(config.wrap_min(), 70);
    let config = Config {
        width: 8,
        ..Config::default()
    };
    assert_eq!(config.wrap_min(), 1);
}
