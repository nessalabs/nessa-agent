//! Stage-scoped on-disk roots for client data ([ADR 0005](../../docs/adr/done/0005-stage-scoped-local-data.md)).
//!
//! `prod` uses the bare app config directory. Every other stage string gets a
//! subdirectory. Optional `NESSA_INSTANCE` further isolates worktrees/sandboxes.
//! The stage is embedded by the build; a runtime override can only confirm it,
//! never silently move a bundle into another namespace.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

const ENV_STAGE: &str = "NESSA_STAGE";
const ENV_INSTANCE: &str = "NESSA_INSTANCE";
const BUNDLE_STAGE: &str = env!("NESSA_BUNDLE_STAGE");

/// A runtime override that disagrees with the stage recorded in this bundle.
#[derive(Debug, PartialEq, Eq)]
pub struct StageMismatch {
    bundle: String,
    runtime: String,
}

impl std::fmt::Display for StageMismatch {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "desktop startup refused: bundle stage {:?}, runtime NESSA_STAGE {:?}",
            self.bundle, self.runtime
        )
    }
}

impl std::error::Error for StageMismatch {}

/// Directory under which `settings.json`, `shortcuts.json`, and later stores live.
pub fn config_root(app: &AppHandle, stage: &str) -> Option<PathBuf> {
    let base = app.path().app_config_dir().ok()?;
    match namespace_segment(stage, process_instance().as_deref()) {
        None => Some(base),
        Some(segment) => Some(base.join(segment)),
    }
}

/// The stage embedded in this bundle, unless an equal runtime override confirms it.
pub fn process_stage() -> Result<String, StageMismatch> {
    let runtime = std::env::var_os(ENV_STAGE).map(|value| {
        value
            .into_string()
            .unwrap_or_else(|value| value.to_string_lossy().into_owned())
    });
    resolve_stage(BUNDLE_STAGE, runtime.as_deref())
}

fn process_instance() -> Option<String> {
    std::env::var(ENV_INSTANCE).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn resolve_stage(bundle: &str, runtime: Option<&str>) -> Result<String, StageMismatch> {
    if let Some(runtime) = runtime {
        if runtime.is_empty() || runtime.trim() != runtime || runtime != bundle {
            return Err(StageMismatch {
                bundle: bundle.to_owned(),
                runtime: runtime.to_owned(),
            });
        }
    }
    Ok(bundle.to_owned())
}

/// `None` means use the bare config dir (`prod` only).
fn namespace_segment(stage: &str, instance: Option<&str>) -> Option<String> {
    if stage.trim().eq_ignore_ascii_case("prod") {
        return None;
    }
    let stage_seg = sanitize_segment(stage);
    match instance.map(sanitize_segment).filter(|s| !s.is_empty()) {
        Some(instance_seg) => Some(format!("{stage_seg}-{instance_seg}")),
        None => Some(stage_seg),
    }
}

/// Filesystem-safe single path segment: letters, digits, `.`, `_`, `-`.
/// Path separators split into joined parts; `.` / `..` parts are dropped.
/// Empty input becomes `unnamed`.
fn sanitize_segment(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in raw.split(['/', '\\']) {
        let mut cleaned = String::with_capacity(part.len());
        for ch in part.chars() {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                cleaned.push(ch);
            } else {
                cleaned.push('-');
            }
        }
        let trimmed = cleaned.trim_matches('-');
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    if parts.is_empty() {
        String::from("unnamed")
    } else {
        parts.join("-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prod_uses_bare_root_even_with_instance() {
        assert_eq!(namespace_segment("prod", None), None);
        assert_eq!(namespace_segment("prod", Some("feat")), None);
        assert_eq!(namespace_segment("PROD", None), None);
    }

    #[test]
    fn open_stage_names_get_their_own_segment() {
        assert_eq!(namespace_segment("dev", None).as_deref(), Some("dev"));
        assert_eq!(
            namespace_segment("dogfood", None).as_deref(),
            Some("dogfood")
        );
        assert_eq!(namespace_segment("alpha", None).as_deref(), Some("alpha"));
    }

    #[test]
    fn instance_joins_with_a_dash() {
        assert_eq!(
            namespace_segment("dev", Some("feat-tabs")).as_deref(),
            Some("dev-feat-tabs")
        );
    }

    #[test]
    fn sanitize_rejects_path_traversal_shapes() {
        assert_eq!(sanitize_segment(".."), "unnamed");
        assert_eq!(sanitize_segment("a/b"), "a-b");
        assert_eq!(sanitize_segment("feat/tabs"), "feat-tabs");
        assert_eq!(
            namespace_segment("../x", Some("y/z")).as_deref(),
            Some("x-y-z")
        );
    }

    #[test]
    fn prod_is_case_insensitive_for_bare_path() {
        assert_eq!(namespace_segment("Prod", None), None);
    }

    #[test]
    fn absent_or_equal_runtime_stage_uses_the_bundle_stage() {
        assert_eq!(resolve_stage("dev", None), Ok("dev".into()));
        assert_eq!(resolve_stage("prod", Some("prod")), Ok("prod".into()));
    }

    #[test]
    fn blank_or_different_runtime_stage_names_both_values() {
        for runtime in ["", " dev ", "prod"] {
            let error = resolve_stage("dev", Some(runtime)).unwrap_err();
            assert_eq!(error.bundle, "dev");
            assert_eq!(error.runtime, runtime);
            let message = error.to_string();
            assert!(message.contains("bundle stage \"dev\""));
            assert!(message.contains(&format!("runtime NESSA_STAGE {runtime:?}")));
        }
    }
}
