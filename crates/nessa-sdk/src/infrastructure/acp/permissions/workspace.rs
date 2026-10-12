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
use crate::infrastructure::workspace_paths::WorkspacePaths;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

/// How long path classification may wait on the filesystem. A canonicalize
/// that has not returned by then is the deny class, so a stuck mount cannot
/// hold the worker that also polls close and deadlines
/// (`a_stuck_filesystem_is_the_deny_class_and_does_not_hold_classification`).
pub(crate) const PATH_RESOLUTION_BOUND: Duration = Duration::from_millis(250);

/// The deadline class for a review of `kind` touching `locations` in `workspace`.
///
/// Read and search are read-only only when every target path canonicalizes to
/// a path inside `workspace`. No path, a missing file, a symlink that leaves,
/// or any other kind is [`PermissionToolClass::Other`]. The worker classifies
/// through [`bounded_review_class`]; this wrapper is what the tests call.
#[cfg(test)]
pub(crate) fn review_class(
    kind: Option<ToolKind>,
    locations: &Option<Vec<FileLocation>>,
    workspace: &Path,
) -> PermissionToolClass {
    review_class_with(kind, locations, workspace, &mut |path| {
        path.canonicalize().ok()
    })
}

/// Classify `kind` and `locations` by resolving paths through `canonicalize`.
///
/// `canonicalize` is the filesystem read. The worker passes one that runs off
/// its task and gives up at [`PATH_RESOLUTION_BOUND`]. A path that does not
/// resolve is outside the read-only class.
pub(crate) fn review_class_with(
    kind: Option<ToolKind>,
    locations: &Option<Vec<FileLocation>>,
    workspace: &Path,
    canonicalize: &mut dyn FnMut(&Path) -> Option<PathBuf>,
) -> PermissionToolClass {
    if !matches!(kind, Some(ToolKind::Read | ToolKind::Search)) {
        return PermissionToolClass::Other;
    }
    let Some(locations) = locations.as_ref().filter(|paths| !paths.is_empty()) else {
        return PermissionToolClass::Other;
    };
    let Some(workspace) = canonicalize(workspace) else {
        return PermissionToolClass::Other;
    };
    if locations
        .iter()
        .all(|location| path_inside(location.path().as_str(), &workspace, canonicalize))
    {
        PermissionToolClass::ReadOnly
    } else {
        PermissionToolClass::Other
    }
}

/// Classify off the worker task through the injected `paths`. A filesystem
/// read that outlasts [`PATH_RESOLUTION_BOUND`] is the deny class.
///
/// `in_flight` is this worker's one classification. The permit stays with the
/// read until the read returns, including after the wait has given up, so a
/// later review does not start another
/// (`a_classification_still_running_is_not_started_again`).
pub(crate) async fn bounded_review_class(
    in_flight: &Arc<Semaphore>,
    kind: Option<ToolKind>,
    locations: Option<Vec<FileLocation>>,
    workspace: PathBuf,
    paths: Arc<dyn WorkspacePaths>,
) -> PermissionToolClass {
    let Ok(permit) = Arc::clone(in_flight).try_acquire_owned() else {
        return PermissionToolClass::Other;
    };
    let work = move || {
        let _permit = permit;
        review_class_with(kind, &locations, &workspace, &mut |path| {
            paths.resolve(path)
        })
    };
    match tokio::time::timeout(PATH_RESOLUTION_BOUND, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(class)) => class,
        _ => PermissionToolClass::Other,
    }
}

fn path_inside(
    raw: &str,
    workspace: &Path,
    canonicalize: &mut dyn FnMut(&Path) -> Option<PathBuf>,
) -> bool {
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        PathBuf::from(path)
    } else {
        workspace.join(path)
    };
    let Some(canonical) = canonicalize(&joined) else {
        return false;
    };
    canonical.starts_with(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_execution::tools::FilePath;
    use std::fs;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::time::timeout;

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

    #[tokio::test]
    async fn classification_reads_paths_only_through_the_injected_resolver() {
        // Neither path exists on disk; only the resolver says where they are.
        let workspace = std::env::temp_dir().join("nessa-absent-workspace");
        let elsewhere = std::env::temp_dir().join("nessa-elsewhere");
        let budget = Arc::new(Semaphore::new(1));
        let read = |name: &str| {
            let elsewhere = elsewhere.clone();
            super::bounded_review_class(
                &budget,
                Some(ToolKind::Read),
                Some(vec![location(&workspace.join(name))]),
                workspace.clone(),
                Arc::new(move |path: &Path| {
                    Some(if path.ends_with("link") {
                        elsewhere.join("secret")
                    } else {
                        path.to_path_buf()
                    })
                }),
            )
        };
        assert!(!workspace.exists());
        assert_eq!(read("notes.txt").await, PermissionToolClass::ReadOnly);
        assert_eq!(read("link").await, PermissionToolClass::Other);
    }

    #[tokio::test]
    async fn a_stuck_filesystem_is_the_deny_class_and_does_not_hold_classification() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let inside = at(&workspace, "notes.txt");
        let locations = Some(vec![location(&inside)]);
        let budget = Arc::new(Semaphore::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_read = Arc::clone(&stop);
        let started = std::time::Instant::now();
        let class = super::bounded_review_class(
            &budget,
            Some(ToolKind::Read),
            locations,
            workspace,
            Arc::new(move |_: &Path| {
                let began = std::time::Instant::now();
                while began.elapsed() < Duration::from_secs(5) {
                    if stop_for_read.load(Ordering::Relaxed) {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                None
            }),
        )
        .await;
        assert_eq!(class, PermissionToolClass::Other);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "classification waited on the filesystem for {:?}",
            started.elapsed()
        );
        stop.store(true, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn a_classification_still_running_is_not_started_again() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let inside = at(&workspace, "notes.txt");
        let budget = Arc::new(Semaphore::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_read = Arc::clone(&stop);
        let first = {
            let budget = Arc::clone(&budget);
            let locations = Some(vec![location(&inside)]);
            let workspace = workspace.clone();
            tokio::spawn(async move {
                super::bounded_review_class(
                    &budget,
                    Some(ToolKind::Read),
                    locations,
                    workspace,
                    Arc::new(move |_: &Path| {
                        while !stop_for_read.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        None
                    }),
                )
                .await
            })
        };
        timeout(Duration::from_secs(1), async {
            loop {
                if budget.available_permits() == 0 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first classification never took its permit");
        let started = std::time::Instant::now();
        let second = super::bounded_review_class(
            &budget,
            Some(ToolKind::Read),
            Some(vec![location(&inside)]),
            workspace,
            Arc::new(|_: &Path| Some(std::path::PathBuf::from("/"))),
        )
        .await;
        assert_eq!(second, PermissionToolClass::Other);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "a second classification waited {:?}",
            started.elapsed()
        );
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            timeout(Duration::from_secs(2), first)
                .await
                .expect("the first classification did not finish")
                .expect("the first classification panicked"),
            PermissionToolClass::Other
        );
    }
}
