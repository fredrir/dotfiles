use std::error::Error;
use std::fs;
use std::path::PathBuf;

use dotfile_test_support::materialize_m2_fixture;

fn main() -> Result<(), Box<dyn Error>> {
    let mut output = Vec::new();
    for argument in std::env::args_os().skip(1) {
        let path = PathBuf::from(argument);
        let old = fs::read_to_string(&path)?;
        let value = materialize_m2_fixture(&path)?;
        let mut new = serde_json::to_string_pretty(&value)?;
        new.push('\n');
        output.push(serde_json::json!({
            "path": path,
            "old": old,
            "new": new,
        }));
    }
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
