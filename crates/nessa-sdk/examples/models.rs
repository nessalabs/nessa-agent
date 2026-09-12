//! Inspect the full catalog or select one entry without provider credentials.
use nessa_sdk::infrastructure::model_metadata_json::load_catalog;
use std::{error::Error, fs::File};

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    if let Err(error) = run() {
        tracing::error!(%error, "Catalog example failed");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 && args.len() != 3 {
        return Err("usage: models <catalog.json> [<provider> <model-id>]".into());
    }
    let catalog = load_catalog(File::open(&args[0])?)?;
    let data = if args.len() == 3 {
        serde_json::to_string_pretty(&catalog.select(&args[1], &args[2])?)?
    } else {
        serde_json::to_string_pretty(&serde_json::json!({
            "verifiedOn": catalog.verified_on(), "models": catalog.models()
        }))?
    };
    tracing::info!(catalog = %data, "Model metadata");
    Ok(())
}
