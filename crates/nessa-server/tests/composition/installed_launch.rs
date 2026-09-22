//! What the desktop asks the store before it writes a launch.
//!
//! Driven through a substitute store rather than a real install: the question
//! here is what composition does with each answer, and the answers a real store
//! is hardest to put into — a record that disagrees with the pin, a store that
//! cannot be read at all — are exactly the ones that matter.
use std::path::PathBuf;

use super::*;

use crate::agent_install::application::{RuntimeStore, StagedArchive, StoreFailure};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, HostPlatform, PinnedRelease, ReleasePlatform,
};
use crate::agent_install::infrastructure::releases_for;
use crate::composition::desktop::bundled_launch;

/// A store that answers however a test needs, and records what it was asked.
struct Answers {
    installed: Result<Option<PathBuf>, StoreFailure>,
    asked: std::sync::Mutex<Vec<(String, String)>>,
}

impl Answers {
    fn saying(installed: Result<Option<PathBuf>, StoreFailure>) -> Self {
        Self {
            installed,
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }
}

// Only `installed` is reached from a launch. The rest of the port belongs to
// installing, and a launch that called any of it would be doing something this
// file exists to say it must not — so they refuse rather than pretend.
impl RuntimeStore for Answers {
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        self.asked.lock().unwrap().push((
            agent.as_str().to_owned(),
            release.version().as_str().to_owned(),
        ));
        self.installed.clone()
    }

    fn stage(&self, _agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        unreachable!("a launch does not stage a download")
    }

    fn digest(&self, _staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        unreachable!("a launch does not measure an archive")
    }

    fn publish(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
        _staged: &mut StagedArchive,
    ) -> Result<PathBuf, StoreFailure> {
        unreachable!("a launch does not install anything")
    }

    fn discard(&self, _staged: StagedArchive) {
        unreachable!("a launch stages nothing to discard")
    }
}

/// The agent the desktop does not bundle, which is the only kind this resolves.
fn unbundled() -> Option<AgentId> {
    AgentId::ALL
        .iter()
        .copied()
        .find(|agent| bundled_launch(*agent).is_none())
}

/// That agent, and a machine one of its pinned builds runs on.
///
/// Not `host_platform()`. Whether a launch is resolved at all depends on the
/// machine the test runs on — Opencode is pinned for macOS and Linux and not
/// for Windows — so a test that asked the real host asserted the behaviour on
/// two of three CI runners and asserted the *absence* of pins on the third. It
/// failed there, which was the honest outcome of a question it should not have
/// been asking: what these tests are about is what composition does with each
/// answer the store gives, and that is the same everywhere.
///
/// Derived from a pin rather than written down, so it keeps meaning "a machine
/// this build serves" when the pinned platforms change. `None` is a build that
/// pins nothing for this agent anywhere, which the tests below have nothing to
/// say about.
fn served() -> Option<(AgentId, HostPlatform)> {
    let agent = unbundled()?;
    let name = AgentName::parse(agent.name()).expect("an agent name is a pin file name");
    let release = releases_for(&name)
        .expect("the pins parse")
        .into_iter()
        .next()?;
    let host = HostPlatform::new(
        release.platform().clone(),
        release.requirements().libc(),
        release.requirements().avx2(),
    );
    Some((agent, host))
}

#[test]
fn an_installed_runtime_is_the_launch() {
    let Some((agent, host)) = served() else {
        return;
    };
    let executable = PathBuf::from("/data/agents/opencode/versions/1.18.31/abc/opencode");
    let store = Answers::saying(Ok(Some(executable.clone())));

    let launch = installed_launch(agent, &host, &store).expect("the pins are readable");

    assert_eq!(launch, Some(executable));
}

#[test]
fn nothing_installed_is_no_launch() {
    let Some((agent, host)) = served() else {
        return;
    };
    let store = Answers::saying(Ok(None));

    let launch = installed_launch(agent, &host, &store).expect("the pins are readable");

    assert_eq!(launch, None);
}

#[test]
fn a_machine_no_build_runs_on_is_no_launch_and_no_question() {
    // The other half of the one above, and what the Windows runner was really
    // exercising: no pinned build fits, so there is nothing to ask the store
    // about. Asked anyway, the store would answer about some other machine's
    // artifact.
    let Some((agent, _)) = served() else { return };
    let elsewhere = HostPlatform::new(
        ReleasePlatform::new("plan9", "sparc64").expect("a usable platform"),
        None,
        false,
    );
    let store = Answers::saying(Ok(Some(PathBuf::from("/data/agents/opencode/x/opencode"))));

    let launch = installed_launch(agent, &elsewhere, &store).expect("the pins are readable");

    assert_eq!(launch, None);
    assert!(
        store.asked.lock().unwrap().is_empty(),
        "the store was asked about a machine no pinned build runs on"
    );
}

#[test]
fn a_store_that_cannot_answer_is_no_launch_rather_than_a_failure() {
    // One optional agent's unreadable record must not decide whether the
    // gateway starts. Claude and Codex are bundled and unaffected by whatever
    // is wrong here, and a person told "the gateway would not start" would have
    // no way to learn which of three agents was at fault.
    let Some((agent, host)) = served() else {
        return;
    };
    let store = Answers::saying(Err(StoreFailure::Unreadable(
        "the record could not be read".into(),
    )));

    let launch = installed_launch(agent, &host, &store)
        .expect("an unreadable store is an answer, not a fault");

    assert_eq!(launch, None);
}

#[test]
fn the_store_is_asked_about_the_release_this_build_pins() {
    // The whole reason this is resolved at every start. Asking "what is
    // installed" would hand back a runtime from an earlier pin — still on the
    // disk, still launchable — and the agent Nessa tested would be replaced by
    // one it never saw.
    let Some((agent, host)) = served() else {
        return;
    };
    let store = Answers::saying(Ok(None));

    let _ = installed_launch(agent, &host, &store);

    let asked = store.asked.lock().unwrap();
    let (name, version) = asked.first().expect("a served machine asks the store");
    assert_eq!(name, agent.name());
    assert!(
        !version.is_empty(),
        "the store is asked about a named version, not about the agent in general"
    );
}

#[test]
fn the_runtime_is_launched_as_an_acp_server() {
    // Recorded from `opencode acp`, which is what the contract fixtures in
    // `nessa-sdk` were taken from. An executable launched without it starts the
    // interactive program instead and never speaks the protocol.
    let Some(agent) = unbundled() else { return };
    assert_eq!(installed_arguments(agent), vec!["acp".to_string()]);
}
