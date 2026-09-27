mod build_stage;

use std::{collections::BTreeSet, env, fs, path::PathBuf};

use serde_json::Value;
use tauri_utils::{config::parse::read_from, platform::Target};

fn main() {
    println!("cargo:rerun-if-env-changed=NESSA_STAGE");
    println!("cargo:rerun-if-env-changed=VITE_NESSA_STAGE");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rerun-if-changed=../protocol/defaults/gateway-ports.json");

    let ports = fs::read_to_string("../protocol/defaults/gateway-ports.json")
        .expect("gateway port table must be readable");
    let document: Value =
        serde_json::from_str(&ports).expect("gateway port table must be valid JSON");
    let known = document["stages"]
        .as_object()
        .expect("gateway port table must contain a stages object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let host = environment_stage("NESSA_STAGE");
    let ui = environment_stage("VITE_NESSA_STAGE");
    let fallback = if env::var("PROFILE").as_deref() == Ok("release") {
        "prod"
    } else {
        "dev"
    };
    let stage = build_stage::resolve(&known, host.as_deref(), ui.as_deref(), fallback)
        .unwrap_or_else(|error| panic!("desktop bundle has no valid stage: {error}"));
    if env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_some() {
        verify_frontend_stage(&known, &stage);
    }
    println!("cargo:rustc-env=NESSA_BUNDLE_STAGE={stage}");

    let attributes = tauri_build::Attributes::new().plugin(
        "dev-console",
        tauri_build::InlinedPlugin::new().commands(&["forward_webview_console"]),
    );
    tauri_build::try_build(attributes).expect("failed to build Tauri application metadata")
}

fn verify_frontend_stage(known: &BTreeSet<String>, bundle: &str) {
    let record = frontend_stage_record();
    let index = build_stage::frontend_index(&record)
        .expect("frontend stage record must have a containing asset directory");
    println!("cargo:rerun-if-changed={}", record.display());
    println!("cargo:rerun-if-changed={}", index.display());
    let index_metadata = fs::metadata(&index).unwrap_or_else(|error| {
        panic!(
            "frontend entry point is absent at {}: {error}. Build the frontend into the effective Tauri build.frontendDist before Cargo",
            index.display()
        )
    });
    if !index_metadata.is_file() {
        panic!(
            "frontend entry point at {} must be a file. Build the frontend into the effective Tauri build.frontendDist before Cargo",
            index.display()
        );
    }
    let source = fs::read_to_string(&record).unwrap_or_else(|error| {
        panic!(
            "frontend build stage record is absent at {}: {error}. Run the desktop build command so the UI is built before Cargo",
            record.display()
        )
    });
    let document: Value = serde_json::from_str(&source).unwrap_or_else(|error| {
        panic!(
            "frontend build stage record at {} is invalid: {error}",
            record.display()
        )
    });
    let frontend = document["stage"].as_str().unwrap_or_else(|| {
        panic!(
            "frontend build stage record at {} must contain a stage string",
            record.display()
        )
    });
    build_stage::verify_frontend(known, bundle, frontend)
        .unwrap_or_else(|error| panic!("refusing to embed mismatched frontend assets: {error}"));
}

fn frontend_stage_record() -> PathBuf {
    let target = env::var("TARGET").expect("Cargo must provide TARGET to the build script");
    let target = Target::from_triple(&target);
    let current_dir = env::current_dir().expect("desktop crate directory must be available");
    let (config, config_paths) = read_from(target, &current_dir)
        .unwrap_or_else(|error| panic!("Tauri config must be readable: {error}"));
    for path in config_paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let override_config = env::var("TAURI_CONFIG").ok();
    build_stage::frontend_stage_record(config, override_config.as_deref()).unwrap_or_else(|error| {
        panic!("desktop bundle has no valid frontend stage record: {error}")
    })
}

fn environment_stage(name: &str) -> Option<String> {
    env::var_os(name).map(|value| {
        value
            .into_string()
            .unwrap_or_else(|_| panic!("{name} must be valid Unicode"))
    })
}
