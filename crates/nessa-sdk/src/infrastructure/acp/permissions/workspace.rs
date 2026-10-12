//! Whether a read or a search stays inside the session workspace.
//!
//! Auto-allow is only for that case. A path that cannot be resolved, that
//! climbs out with `..`, or that follows a symlink outside the workspace is
//! the deny class. The bytes a read returns are sent to the model provider;
//! staying inside the workspace is not a claim that they stay on the machine.
use crate::domain::agent_execution::{
    permissions::PermissionToolClass,
    tools::{FileLocation, ToolKind},
};
use std::path::{Path, PathBuf};

/// The deadline class for a review of `kind` touching `locations` in `workspace`.
///
/// Read and search are read-only only when every target path canonicalizes to
/// a path inside `workspace`. No path, a missing file, a symlink that leaves,
/// or any other kind is [`PermissionToolClass::Other`].
pub(crate) fn review_class(
    kind: Option<ToolKind>,
    locations: &Option<Vec<FileLocation>>,
    workspace: &Path,
) -> PermissionToolClass {
    if !matches!(kind, Some(ToolKind::Read | ToolKind::Search)) {
        return PermissionToolClass::Other;
    }
    let Some(locations) = locations.as_ref().filter(|paths| !paths.is_empty()) else {
        return PermissionToolClass::Other;
    };
    let Ok(workspace) = workspace.canonicalize() else {
        return PermissionToolClass::Other;
    };
    if locations
        .iter()
        .all(|location| path_inside(location.path().as_str(), &workspace))
    {
        PermissionToolClass::ReadOnly
    } else {
        PermissionToolClass::Other
    }
}

fn path_inside(raw: &str, workspace: &Path) -> bool {
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        PathBuf::from(path)
    } else {
        workspace.join(path)
    };
    let Ok(canonical) = joined.canonicalize() else {
        return false;
    };
    canonical.starts_with(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_execution::tools::FilePath;
    use std::fs;

    fn location(path: &Path) -> FileLocation {
        FileLocation::new(FilePath::new(path.to_str().unwrap()).unwrap(), None)
    }

    fn link_outside(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }

    fn at(root: &Path, name: &str) -> std::path::PathBuf {
        let path = root.join(name);
        fs::write(&path, "contents").unwrap();
        path
    }

    #[test]
    fn only_an_in_workspace_read_or_search_is_read_only() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let inside = at(&workspace, "notes.txt");
        let outside_dir = root.path().join("home");
        fs::create_dir(&outside_dir).unwrap();
        let ssh = at(&outside_dir, ".ssh");
        let locations = Some(vec![location(&inside)]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &locations, &workspace),
            PermissionToolClass::ReadOnly
        );
        assert_eq!(
            review_class(Some(ToolKind::Search), &locations, &workspace),
            PermissionToolClass::ReadOnly
        );
        let relative = Some(vec![FileLocation::new(
            FilePath::new("notes.txt").unwrap(),
            None,
        )]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &relative, &workspace),
            PermissionToolClass::ReadOnly
        );
        let climbed = workspace.join("sub");
        fs::create_dir(&climbed).unwrap();
        let stayed = Some(vec![FileLocation::new(
            FilePath::new("sub/../notes.txt").unwrap(),
            None,
        )]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &stayed, &workspace),
            PermissionToolClass::ReadOnly
        );

        let ssh_locations = Some(vec![location(&ssh)]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &ssh_locations, &workspace),
            PermissionToolClass::Other,
            "~/.ssh-style path"
        );
        let climbed_out = Some(vec![FileLocation::new(
            FilePath::new("../home/.ssh").unwrap(),
            None,
        )]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &climbed_out, &workspace),
            PermissionToolClass::Other,
            ".."
        );
        link_outside(&ssh, &workspace.join("looks-inside"));
        let escaped = Some(vec![location(&workspace.join("looks-inside"))]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &escaped, &workspace),
            PermissionToolClass::Other,
            "symlink escape"
        );
        assert_eq!(
            review_class(Some(ToolKind::Read), &None, &workspace),
            PermissionToolClass::Other,
            "no path"
        );
        assert_eq!(
            review_class(Some(ToolKind::Read), &Some(vec![]), &workspace),
            PermissionToolClass::Other
        );
        let mixed = Some(vec![location(&inside), location(&ssh)]);
        assert_eq!(
            review_class(Some(ToolKind::Search), &mixed, &workspace),
            PermissionToolClass::Other
        );
        let missing = Some(vec![FileLocation::new(
            FilePath::new(workspace.join("missing.txt").to_str().unwrap()).unwrap(),
            None,
        )]);
        assert_eq!(
            review_class(Some(ToolKind::Read), &missing, &workspace),
            PermissionToolClass::Other
        );
    }
}
