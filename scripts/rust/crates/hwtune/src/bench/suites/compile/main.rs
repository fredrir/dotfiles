use clap::Parser;
use serde::{Deserialize, Serialize};
#[derive(Parser)]
struct Cli { #[arg(long)] name: Option<String> }
#[derive(Serialize, Deserialize)]
struct Record { name: String, when: String }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let re = regex::Regex::new(r"^[a-z]+$")?;
    let record = Record { name: cli.name.unwrap_or_default(), when: chrono::Utc::now().to_rfc3339() };
    println!("{} {}", serde_json::to_string(&record)?, re.is_match(&record.name));
    Ok(())
}
