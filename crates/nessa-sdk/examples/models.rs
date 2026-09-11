//! Inspect the full catalog or select one entry without provider credentials.
use nessa_sdk::infrastructure::model_metadata_json::load_catalog;
use std::{error::Error, fs::File};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 && args.len() != 3 {
        return Err("usage: models <catalog.json> [<provider> <model-id>]".into());
    }
    let catalog = load_catalog(File::open(&args[0])?)?;
    if args.len() == 3 {
        println!(
            "{}",
            serde_json::to_string_pretty(&catalog.select(&args[1], &args[2])?)?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "verifiedOn": catalog.verified_on(), "models": catalog.models()
            }))?
        );
    }
    Ok(())
}
