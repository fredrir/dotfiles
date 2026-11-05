#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let config = dotfmt_typescript::Config::default();
    let _ = dotfmt_typescript::format(
        Path::new("fuzz.tsx"),
        text,
        &config,
        dotfmt_typescript::Dialect::Tsx,
    );
    let _ = dotfmt_typescript::analyze(
        Path::new("fuzz.tsx"),
        text,
        &config,
        dotfmt_typescript::Dialect::Tsx,
        &dotfmt_typescript::analyze::Options {
            fix: true,
            ..Default::default()
        },
    );
});
