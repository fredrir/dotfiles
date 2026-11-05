#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let config = dotfmt_css::Config::default();
    for dialect in [
        dotfmt_css::Dialect::Css,
        dotfmt_css::Dialect::Scss,
        dotfmt_css::Dialect::Sass,
        dotfmt_css::Dialect::Less,
    ] {
        let _ = dotfmt_css::format(Path::new("fuzz.css"), text, &config, dialect);
    }
});
