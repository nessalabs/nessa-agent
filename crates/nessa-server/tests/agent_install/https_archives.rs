use super::*;
use crate::agent_install::application::{InstallAgentRuntime, RuntimeStore};
use crate::agent_install::infrastructure::{host_platform, release_for, ManagedRuntimes};

/// Install the real pinned Opencode release, over the real network.
///
/// Ignored by default: it fetches around fifty megabytes and depends on the npm
/// registry being reachable, neither of which belongs in a suite that runs on
/// every change. It is checked in because it is the only test that exercises
/// the whole path against the artifact Nessa actually ships a pin for — a fake
/// archive cannot tell you that the pinned digest is right, that the real
/// tarball puts its executable where the pin says, or that what comes out is a
/// binary this machine will run.
///
/// Run it after changing the pin:
///
/// ```text
/// cargo test -p nessa-server --lib -- --ignored installs_the_pinned_release
/// ```
#[test]
#[ignore = "downloads the real release archive"]
fn installs_the_pinned_release() {
    let platform = host_platform();
    let Some(release) = release_for("opencode", &platform).expect("the pinned releases parse")
    else {
        // Not a failure: the pin covers the platforms Nessa installs on, and a
        // developer on another one should not see a red test for it.
        eprintln!("no opencode release pinned for {platform}; nothing to install");
        return;
    };

    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let source = HttpsArchives::new().expect("an https client");

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute("opencode", &release, &platform)
    .expect("the pinned release installs");

    assert!(installed.downloaded);
    assert_eq!(&installed.version, release.version());
    assert!(installed.executable.is_file());

    // The pinned digest is only worth having if it matches what the registry
    // actually serves today. Installing at all proves it did.
    let size = installed
        .executable
        .metadata()
        .expect("reading the installed runtime")
        .len();
    assert!(
        size > 1_000_000,
        "an agent runtime should be a real binary, got {size} bytes"
    );

    // The point of a pin is that the version Nessa tested is the version that
    // runs. Nothing so far has asked the binary itself: the digest proves the
    // archive was the measured one, not that what came out of it launches or
    // agrees about what it is. Running it closes both gaps at once — and on
    // macOS it is also what would catch a binary the system refuses to execute.
    let reported = std::process::Command::new(&installed.executable)
        .arg("--version")
        .output()
        .expect("the installed runtime can be launched");
    let version = String::from_utf8_lossy(&reported.stdout);
    assert!(
        version.contains(release.version().as_str()),
        "the installed runtime reports {version:?}, not the pinned {}",
        release.version()
    );

    // Asking again must be free. This is what stops a first-run surface from
    // downloading a hundred megabytes because somebody pressed the button twice.
    let again = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute("opencode", &release, &platform)
    .expect("an installed runtime is reported as installed");
    assert!(!again.downloaded);
    assert_eq!(again.executable, installed.executable);

    // And the record survives a fresh store over the same directory, which is
    // what happens on the next launch of the app.
    let reopened = ManagedRuntimes::new(root.path());
    let record = reopened
        .installed("opencode")
        .expect("the store can be read")
        .expect("the runtime is still installed");
    assert_eq!(&record.version, release.version());
}
