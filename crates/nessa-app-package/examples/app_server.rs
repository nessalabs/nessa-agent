//! Run a dynamic app's server as its app host does, without a window: for
//! checks that drive the server from a script (the conversation stress
//! step `dynamicApps`).
//!
//! ```text
//! app_server --package <inline.json> --folder <launcher folder> --client <app-client.mjs>
//!   <inline.json>   {"manifest": base64, "assets": [{"path", "bytes": base64}]}
//!   <folder>        holds credential.json, as Nessa's host keeps it
//! prints {"origin": "http://127.0.0.1:<port>", "token": "…"} once it answers,
//! then runs until its stdin closes
//! ```
use std::io::Read;
use std::path::PathBuf;

use nessa_app_package::{start_server, verify_inline_package, InlineAsset, InlinePackage};
use serde_json::Value;

fn main() -> Result<(), String> {
    let mut package = None;
    let mut folder = None;
    let mut client = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} names nothing"))?;
        match arg.as_str() {
            "--package" => package = Some(PathBuf::from(value)),
            "--folder" => folder = Some(PathBuf::from(value)),
            "--client" => client = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (Some(package), Some(folder), Some(client)) = (package, folder, client) else {
        return Err("usage: app_server --package <file> --folder <dir> --client <file>".into());
    };
    let read = |path: &PathBuf| {
        std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let inline: Value =
        serde_json::from_slice(&read(&package)?).map_err(|error| error.to_string())?;
    let text = |value: &Value| value.as_str().map(str::to_owned).ok_or("not text");
    let inline = InlinePackage {
        manifest_base64: text(&inline["manifest"])?,
        assets: inline["assets"]
            .as_array()
            .ok_or("no assets")?
            .iter()
            .map(|asset| {
                Ok(InlineAsset {
                    path: text(&asset["path"])?,
                    bytes_base64: text(&asset["bytes"])?,
                })
            })
            .collect::<Result<_, &str>>()?,
    };
    let verified = verify_inline_package(&inline).map_err(|refusal| format!("{refusal:?}"))?;
    let server = start_server(&verified, &folder, &read(&client)?, None)
        .map_err(|refusal| refusal.to_string())?;
    println!(
        "{}",
        serde_json::json!({ "origin": server.origin(), "token": server.token() })
    );
    // Until whoever started this lets go.
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
    drop(server);
    Ok(())
}
