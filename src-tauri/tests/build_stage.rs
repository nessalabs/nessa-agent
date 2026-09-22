#[path = "../build_stage.rs"]
mod build_stage;

use std::collections::BTreeSet;

use serde_json::json;

fn known_stages() -> BTreeSet<String> {
    let document: serde_json::Value =
        serde_json::from_str(include_str!("../../protocol/defaults/gateway-ports.json"))
            .expect("the real gateway port table must parse");
    document["stages"]
        .as_object()
        .expect("the real gateway port table must name stages")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn bundle_stage_defaults_follow_the_cargo_profile() {
    let known = known_stages();
    assert_eq!(
        build_stage::resolve(&known, None, None, "dev"),
        Ok("dev".into())
    );
    assert_eq!(
        build_stage::resolve(&known, None, None, "prod"),
        Ok("prod".into())
    );
}

#[test]
fn explicit_host_and_ui_stages_must_agree() {
    let known = known_stages();
    assert_eq!(
        build_stage::resolve(&known, Some("dev"), Some("dev"), "prod"),
        Ok("dev".into())
    );
    assert_eq!(
        build_stage::resolve(&known, Some("prod"), Some("dev"), "prod"),
        Err(build_stage::BuildStageError::Conflict {
            host: "prod".into(),
            ui: "dev".into(),
        })
    );
}

#[test]
fn blank_and_unknown_build_stages_are_refused() {
    let known = known_stages();
    assert!(matches!(
        build_stage::resolve(&known, Some(""), None, "dev"),
        Err(build_stage::BuildStageError::Invalid { .. })
    ));
    assert_eq!(
        build_stage::resolve(&known, Some("staging"), None, "dev"),
        Err(build_stage::BuildStageError::Unknown("staging".into()))
    );
}

#[test]
fn prebuilt_frontend_must_record_the_same_stage_as_the_host_bundle() {
    let known = known_stages();
    assert_eq!(
        build_stage::verify_frontend(&known, "alpha", "alpha"),
        Ok(())
    );
    assert_eq!(
        build_stage::verify_frontend(&known, "prod", "alpha"),
        Err(build_stage::BuildStageError::FrontendMismatch {
            bundle: "prod".into(),
            frontend: "alpha".into(),
        })
    );
    assert_eq!(
        build_stage::verify_frontend(&known, "prod", "staging"),
        Err(build_stage::BuildStageError::Unknown("staging".into()))
    );
}

#[test]
fn frontend_stage_record_follows_the_effective_tauri_dist() {
    let config = json!({ "build": { "frontendDist": "../dist" } });
    assert_eq!(
        build_stage::frontend_stage_record(config.clone(), None),
        Ok("../dist/nessa-stage.json".into())
    );
    assert_eq!(
        build_stage::frontend_stage_record(
            config,
            Some(r#"{"build":{"frontendDist":"../alpha-dist"}}"#),
        ),
        Ok("../alpha-dist/nessa-stage.json".into())
    );
}

#[test]
fn contradictory_tauri_dist_overrides_are_refused() {
    let config = json!({ "build": { "frontendDist": "../dist" } });
    assert_eq!(
        build_stage::frontend_stage_record(config.clone(), Some(r#"{"build":null}"#)),
        Err(build_stage::BuildStageError::MissingFrontendDist)
    );
    assert_eq!(
        build_stage::frontend_stage_record(
            config,
            Some(r#"{"build":{"frontendDist":["../dist/index.html"]}}"#),
        ),
        Err(build_stage::BuildStageError::UnsupportedFrontendDist)
    );
}
