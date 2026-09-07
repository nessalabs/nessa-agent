//! Server-owned data paths. No Tauri dependency and no process environment reads.
use super::EnvironmentError;
use std::path::{Path, PathBuf};

/// Resolve the stage/instance namespace beneath an absolute data root.
/// Explicit roots support isolated CI runs; default local data lives in ~/.nessa.
/// Reject ambiguous segments rather than allowing two instances to share credentials.
pub(super) fn auth_directory(
    data_dir: Option<&str>,
    home: Option<&str>,
    stage: &str,
    instance: Option<&str>,
) -> Result<Option<PathBuf>, EnvironmentError> {
    for segment in std::iter::once(stage).chain(instance) {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(EnvironmentError::Backend("stage and NESSA_INSTANCE must be nonempty alphanumeric, hyphen or underscore segments"));
        }
    }
    let base = match (data_dir, home) {
        (Some(value), _) if Path::new(value).is_absolute() => PathBuf::from(value),
        (Some(_), _) => {
            return Err(EnvironmentError::Backend(
                "NESSA_DATA_DIR must be an absolute path",
            ))
        }
        (None, Some(value)) if Path::new(value).is_absolute() => Path::new(value).join(".nessa"),
        (None, Some(_)) => return Err(EnvironmentError::Backend("HOME must be an absolute path")),
        (None, None) => return Ok(None),
    };
    let root = if stage == "prod" {
        base
    } else {
        base.join(stage)
    };
    let root = match instance {
        Some(value) => root.join("instances").join(value),
        None => root,
    };
    Ok(Some(root.join("auth")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_and_instance_are_isolated_including_production() {
        let base = std::env::temp_dir().join("nessa-path-test");
        let path = |stage, instance| {
            auth_directory(base.to_str(), None, stage, instance)
                .unwrap()
                .unwrap()
        };
        assert_eq!(path("prod", None), base.join("auth"));
        assert_ne!(path("prod", None), path("prod", Some("worktree")));
        assert_ne!(path("dev", Some("one")), path("dev", Some("two")));
        assert_ne!(path("dev", None), path("ci", None));
    }

    #[test]
    fn rejects_traversal_and_relative_roots() {
        for instance in ["..", "", "one/two", "one two"] {
            assert!(auth_directory(Some("/data"), None, "dev", Some(instance)).is_err());
        }
        assert!(auth_directory(Some("relative"), None, "dev", None).is_err());
        assert_eq!(auth_directory(None, None, "dev", None).unwrap(), None);
    }
}
