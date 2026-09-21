//! What the harness store answers before, during and after an install.
//!
//! The install itself reaches the network, so it is not run here — what is
//! tested is everything around it: what counts as installed, what a missing
//! manifest does, and that an install already present is not repeated. That
//! last one is what makes this safe to call at every start.
use super::*;

use tempfile::TempDir;

/// The agent these tests install, and where its entry script sits.
fn agent() -> AgentName {
    AgentName::parse("claude").expect("a name")
}

fn version() -> ReleaseVersion {
    ReleaseVersion::parse("0.76.0").expect("a version")
}

const ENTRY: &str = "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js";

/// A root to install into and a bundle to install from.
fn roots() -> (TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("a temporary root");
    let store = root.path().join("agents");
    let manifests = root.path().join("bundle");
    (root, store, manifests)
}

/// Put the two files the application ships for `agent` where it ships them.
fn ship_manifests(manifests: &Path) {
    let harness = manifests.join(agent().as_str());
    std::fs::create_dir_all(&harness).expect("a manifest directory");
    std::fs::write(harness.join("package.json"), "{}").expect("a manifest");
    std::fs::write(harness.join("package-lock.json"), "{}").expect("a lockfile");
}

#[test]
fn nothing_installed_is_nothing_reported() {
    let (_root, store, manifests) = roots();
    let harnesses = NodeHarnesses::new(&store, &manifests);

    assert_eq!(harnesses.installed(&agent(), &version(), ENTRY), None);
}

#[test]
fn a_directory_without_the_entry_script_is_not_an_install() {
    // An interrupted `npm ci` leaves the version directory behind. Answering
    // from the directory would report a runtime that cannot be launched, and
    // the launch would fail later with nothing to say why.
    let (_root, store, manifests) = roots();
    let harnesses = NodeHarnesses::new(&store, &manifests);
    std::fs::create_dir_all(store.join(agent().as_str()).join(version().as_str()))
        .expect("a half-finished install");

    assert_eq!(harnesses.installed(&agent(), &version(), ENTRY), None);
}

#[test]
fn the_entry_script_is_what_makes_it_installed() {
    let (_root, store, manifests) = roots();
    let harnesses = NodeHarnesses::new(&store, &manifests);
    let entry = store
        .join(agent().as_str())
        .join(version().as_str())
        .join(ENTRY);
    std::fs::create_dir_all(entry.parent().expect("a parent")).expect("the tree");
    std::fs::write(&entry, "module.exports = {}").expect("an entry script");

    assert_eq!(
        harnesses.installed(&agent(), &version(), ENTRY),
        Some(entry)
    );
}

#[test]
fn an_install_already_there_is_not_repeated() {
    // The whole reason this is safe on a path that runs at every start: the
    // second call must answer from the disk rather than reach the network. If
    // this regressed, every launch would run `npm ci` — and the failure would
    // look like a slow start rather than a bug.
    let (_root, store, manifests) = roots();
    let harnesses = NodeHarnesses::new(&store, &manifests);
    let entry = store
        .join(agent().as_str())
        .join(version().as_str())
        .join(ENTRY);
    std::fs::create_dir_all(entry.parent().expect("a parent")).expect("the tree");
    std::fs::write(&entry, "module.exports = {}").expect("an entry script");

    // No manifests are shipped here at all. An install that ran would fail on
    // that, so reaching `Ok` proves it did not run.
    let installed = harnesses.install(&agent(), &version(), ENTRY);

    assert_eq!(installed, Ok(entry));
}

#[test]
fn missing_manifests_are_reported_as_the_build_fault_they_are() {
    let (_root, store, manifests) = roots();
    let harnesses = NodeHarnesses::new(&store, &manifests);

    let refused = harnesses.install(&agent(), &version(), ENTRY);

    match refused {
        Err(HarnessFailure::Manifests(path)) => assert!(
            path.contains("package.json"),
            "the refusal names the file that is missing, not just that one is"
        ),
        other => panic!("expected a manifest failure, got {other:?}"),
    }
}

#[test]
fn a_lockfile_alone_is_not_enough_to_install_from() {
    // `npm ci` needs both, and refuses if they disagree. Catching it here means
    // the failure names the missing file rather than arriving as npm's own
    // message about a directory it was pointed at.
    let (_root, store, manifests) = roots();
    ship_manifests(&manifests);
    std::fs::remove_file(manifests.join(agent().as_str()).join("package.json"))
        .expect("removing the manifest");
    let harnesses = NodeHarnesses::new(&store, &manifests);

    assert!(matches!(
        harnesses.install(&agent(), &version(), ENTRY),
        Err(HarnessFailure::Manifests(_))
    ));
}
