use super::super::control::RetirementEvidence;
use super::super::generation::random_generation;
use super::{
    collect, listing, removable, retained_runtimes, Collected, LabelDirectory, Listing,
    RetainedRuntimes, RuntimeVersions,
};
use std::{
    cell::RefCell,
    ffi::OsString,
    fs::{self, DirBuilder},
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, DirBuilderExt},
    },
    path::PathBuf,
};

fn fingerprint(marker: &str) -> String {
    marker.repeat(64 / marker.len())
}

fn only_current() -> RetainedRuntimes {
    RetainedRuntimes::new(&fingerprint("a"), &fingerprint("a"), None, None)
}

/// Records what was asked of the directory, and fails whichever removals a test
/// names, so a failing removal can be told apart from one never attempted.
struct FakeVersions {
    listing: Result<Listing, String>,
    refuse: Vec<String>,
    attempted: RefCell<Vec<String>>,
}
impl FakeVersions {
    /// Goes through the real split, so a fake directory reads like a real one.
    fn holding_entries(entries: Vec<OsString>) -> Self {
        Self {
            listing: Ok(listing(entries)),
            refuse: Vec::new(),
            attempted: RefCell::new(Vec::new()),
        }
    }
    fn holding(names: &[String]) -> Self {
        Self::holding_entries(names.iter().map(OsString::from).collect())
    }
    fn unreadable(error: &str) -> Self {
        Self {
            listing: Err(error.into()),
            refuse: Vec::new(),
            attempted: RefCell::new(Vec::new()),
        }
    }
    fn refusing(mut self, name: &str) -> Self {
        self.refuse.push(name.into());
        self
    }
}
impl RuntimeVersions for FakeVersions {
    fn list(&self) -> Result<Listing, String> {
        self.listing.clone()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.attempted.borrow_mut().push(name.into());
        if self.refuse.iter().any(|refused| refused == name) {
            return Err("permission denied".into());
        }
        Ok(())
    }
}

#[test]
fn the_registered_and_running_versions_are_never_removable() {
    let retained = RetainedRuntimes::new(&fingerprint("a"), &fingerprint("b"), None, None);
    assert!(!removable(&fingerprint("a"), &retained));
    assert!(!removable(&fingerprint("b"), &retained));
    assert!(removable(&fingerprint("c"), &retained));
}

#[test]
fn a_version_named_by_a_pending_request_or_a_fence_is_never_removable() {
    let pending = RetainedRuntimes::new(
        &fingerprint("a"),
        &fingerprint("a"),
        Some(&fingerprint("b")),
        None,
    );
    assert!(!removable(&fingerprint("b"), &pending));
    assert!(removable(&fingerprint("c"), &pending));

    let fenced = RetainedRuntimes::new(
        &fingerprint("a"),
        &fingerprint("a"),
        None,
        Some(&fingerprint("c")),
    );
    assert!(!removable(&fingerprint("c"), &fenced));
    assert!(removable(&fingerprint("b"), &fenced));
}

fn evidence(marker: &str, retired: bool) -> RetirementEvidence {
    RetirementEvidence {
        fingerprint: fingerprint(marker),
        generation: fingerprint("e"),
        retired,
    }
}

#[test]
fn an_unanswered_request_keeps_its_version_and_a_completed_retirement_does_not() {
    let current = fingerprint("a");
    let old = fingerprint("b");

    let pending = retained_runtimes(&current, &current, Some(&evidence("b", false)), None);
    assert!(!removable(&old, &pending));

    // The retiring gateway cleaned up, said so durably, and was booted out.
    let completed = retained_runtimes(&current, &current, None, Some(&evidence("b", true)));
    assert!(removable(&old, &completed));

    // Admitted, but cleanup or audit failed: nothing proved that process gone.
    let uncertain = retained_runtimes(&current, &current, None, Some(&evidence("b", false)));
    assert!(!removable(&old, &uncertain));

    // A completed fence never overrides the versions in use.
    let current_is_fenced = retained_runtimes(&current, &current, None, Some(&evidence("a", true)));
    assert!(!removable(&current, &current_is_fenced));

    assert!(!removable(
        &old,
        &retained_runtimes(&old, &current, None, None)
    ));
    assert!(!removable(
        &old,
        &retained_runtimes(&current, &old, None, None)
    ));
}

#[test]
fn only_names_this_host_writes_are_removable() {
    let retained = only_current();
    for kept in [
        String::new(),
        ".".to_owned(),
        "..".to_owned(),
        ".staging-".to_owned(),
        format!(".staging-{}", "g".repeat(64)),
        format!(".staging-{}", "a".repeat(63)),
        fingerprint("A"),
        "a".repeat(63),
        "a".repeat(65),
        "manifest.json".to_owned(),
        "gateway-runtimes".to_owned(),
        format!("{}.old", fingerprint("b")),
    ] {
        assert!(!removable(&kept, &retained), "{kept} must be kept");
    }
    assert!(removable(
        &format!(".staging-{}", fingerprint("d")),
        &retained
    ));
    assert!(removable(&random_generation().unwrap(), &retained));
    assert!(removable(
        &format!(".staging-{}", random_generation().unwrap()),
        &retained
    ));
}

#[test]
fn an_update_leaves_the_current_version_and_collects_the_generations_behind_it() {
    let versions = FakeVersions::holding(&[
        fingerprint("a"),
        fingerprint("b"),
        fingerprint("c"),
        format!(".staging-{}", fingerprint("d")),
        "unexpected".to_owned(),
    ]);
    // A listing is read in one stable order, whatever the directory returned.
    let expected = vec![
        format!(".staging-{}", fingerprint("d")),
        fingerprint("b"),
        fingerprint("c"),
    ];
    assert_eq!(
        collect(&versions, &only_current()),
        Collected {
            removed: expected.clone(),
            failures: Vec::new(),
        }
    );
    assert_eq!(*versions.attempted.borrow(), expected);
}

#[test]
fn a_pending_retirement_leaves_two_published_versions() {
    let versions = FakeVersions::holding(&[fingerprint("a"), fingerprint("b"), fingerprint("c")]);
    let retained = RetainedRuntimes::new(
        &fingerprint("a"),
        &fingerprint("a"),
        Some(&fingerprint("b")),
        None,
    );
    assert_eq!(
        collect(&versions, &retained).removed,
        vec![fingerprint("c")]
    );
}

#[test]
fn a_refused_removal_is_reported_and_does_not_stop_the_others() {
    let versions =
        FakeVersions::holding(&[fingerprint("b"), fingerprint("c")]).refusing(&fingerprint("b"));
    assert_eq!(
        collect(&versions, &only_current()),
        Collected {
            removed: vec![fingerprint("c")],
            failures: vec![(fingerprint("b"), "permission denied".into())],
        }
    );
}

/// APFS accepts only valid UTF-8, so no test on this machine can create such an
/// entry; the split is exercised through `listing`, the way `staging.rs` tests
/// its own rejection of a name it cannot create either.
#[test]
fn an_entry_with_no_name_is_reported_and_the_obsolete_version_beside_it_still_goes() {
    let odd = OsString::from_vec(vec![0xff]);
    let versions = FakeVersions::holding_entries(vec![
        OsString::from(fingerprint("a")),
        OsString::from(fingerprint("b")),
        odd.clone(),
    ]);
    let collected = collect(&versions, &only_current());

    assert_eq!(collected.removed, vec![fingerprint("b")]);
    assert_eq!(*versions.attempted.borrow(), vec![fingerprint("b")]);
    assert_eq!(collected.failures.len(), 1);
    assert_eq!(collected.failures[0].0, odd.to_string_lossy());
    assert!(collected.failures[0].1.contains("left in place"));
}

#[test]
fn a_failed_retirement_keeps_its_runtime_until_a_later_reconciliation_sees_it_through() {
    let current = fingerprint("a");
    let old = fingerprint("b");

    // The retry's own reconciliation reads the failed attempt's fence before it
    // asks the old gateway again, so it still describes an unfinished
    // retirement even once the replacement is up.
    let during_retry = retained_runtimes(&current, &current, None, Some(&evidence("b", false)));
    let versions = FakeVersions::holding(&[current.clone(), old.clone()]);
    assert_eq!(
        collect(&versions, &during_retry).removed,
        Vec::<String>::new()
    );
    assert!(versions.attempted.borrow().is_empty());

    // The next reconciliation reads the acknowledgement the retry wrote.
    let afterwards = retained_runtimes(&current, &current, None, Some(&evidence("b", true)));
    let versions = FakeVersions::holding(&[current, old.clone()]);
    assert_eq!(collect(&versions, &afterwards).removed, vec![old]);
}

#[test]
fn a_directory_that_cannot_be_read_is_reported_and_removes_nothing() {
    let versions = FakeVersions::unreadable("no such file or directory");
    assert_eq!(
        collect(&versions, &only_current()),
        Collected {
            removed: Vec::new(),
            failures: vec![(String::new(), "no such file or directory".into())],
        }
    );
    assert!(versions.attempted.borrow().is_empty());
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nessa-runtime-prune-{}",
            random_generation().unwrap()
        ));
        nessa_local_storage::create_directory(&path).unwrap();
        Self(path)
    }
    fn version(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::write(path.join("nessa"), b"gateway").unwrap();
        DirBuilder::new()
            .mode(0o700)
            .create(path.join("nested"))
            .unwrap();
        fs::write(path.join("nested/node"), b"node").unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_real_directory_removes_a_published_tree_and_keeps_the_current_one() {
    let fixture = Fixture::new();
    let current = fixture.version(&fingerprint("a"));
    let stale = fixture.version(&fingerprint("b"));
    let attempt = fixture.version(&format!(".staging-{}", fingerprint("c")));
    let directory = LabelDirectory::at(&fixture.0);

    let listed = directory.list().unwrap();
    assert_eq!(listed.passed_over, Vec::new());
    assert_eq!(
        listed.names,
        vec![
            format!(".staging-{}", fingerprint("c")),
            fingerprint("a"),
            fingerprint("b")
        ]
    );

    let collected = collect(&directory, &only_current());
    assert_eq!(collected.failures, Vec::new());
    assert_eq!(collected.removed.len(), 2);
    assert!(current.exists());
    assert!(!stale.exists());
    assert!(!attempt.exists());
}

#[test]
fn the_real_directory_refuses_an_entry_that_is_not_a_directory_it_owns() {
    let fixture = Fixture::new();
    let outside = fixture.version(&fingerprint("a"));
    let link = fixture.0.join(fingerprint("b"));
    symlink(&outside, &link).unwrap();
    let file = fixture.0.join(fingerprint("c"));
    fs::write(&file, b"not a runtime").unwrap();
    let directory = LabelDirectory::at(&fixture.0);

    let collected = collect(&directory, &only_current());
    assert_eq!(collected.removed, Vec::<String>::new());
    assert_eq!(
        collected.failures,
        vec![
            (
                fingerprint("b"),
                "Staged runtime entry is not a directory".to_owned()
            ),
            (
                fingerprint("c"),
                "Staged runtime entry is not a directory".to_owned()
            ),
        ]
    );
    assert!(outside.join("nessa").exists());
    assert!(fs::symlink_metadata(&link).is_ok());
    assert!(file.exists());
}

#[test]
fn a_missing_directory_is_a_reported_failure_rather_than_a_panic() {
    let fixture = Fixture::new();
    let directory = LabelDirectory::at(&fixture.0.join("never-registered"));
    let collected = collect(&directory, &only_current());
    assert_eq!(collected.removed, Vec::<String>::new());
    assert_eq!(collected.failures.len(), 1);
    assert_eq!(collected.failures[0].0, "");
}
