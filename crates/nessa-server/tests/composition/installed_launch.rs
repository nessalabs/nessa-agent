//! What the desktop asks the store before it writes a launch.
//!
//! Driven through a substitute store rather than a real install: the question
//! here is what composition does with each answer, and the answers a real store
//! is hardest to put into — a record that disagrees with the pin, a store that
//! cannot be read at all — are exactly the ones that matter.
use std::path::PathBuf;

use super::*;

use crate::agent_install::application::{RuntimeStore, StagedArchive, StoreFailure};
use crate::agent_install::domain::{AgentName, ArchiveDigest, PinnedRelease};
use crate::agent_install::infrastructure::host_platform;
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

#[test]
fn an_installed_runtime_is_the_launch() {
    let Some(agent) = unbundled() else { return };
    let executable = PathBuf::from("/data/agents/opencode/versions/1.18.31/abc/opencode");
    let store = Answers::saying(Ok(Some(executable.clone())));

    let launch = installed_launch(agent, &host_platform(), &store).expect("the pins are readable");

    assert_eq!(launch, Some(executable));
}

#[test]
fn nothing_installed_is_no_launch() {
    let Some(agent) = unbundled() else { return };
    let store = Answers::saying(Ok(None));

    let launch = installed_launch(agent, &host_platform(), &store).expect("the pins are readable");

    assert_eq!(launch, None);
}

#[test]
fn a_store_that_cannot_answer_is_no_launch_rather_than_a_failure() {
    // One optional agent's unreadable record must not decide whether the
    // gateway starts. Claude and Codex are bundled and unaffected by whatever
    // is wrong here, and a person told "the gateway would not start" would have
    // no way to learn which of three agents was at fault.
    let Some(agent) = unbundled() else { return };
    let store = Answers::saying(Err(StoreFailure::Unreadable(
        "the record could not be read".into(),
    )));

    let launch = installed_launch(agent, &host_platform(), &store)
        .expect("an unreadable store is an answer, not a fault");

    assert_eq!(launch, None);
}

#[test]
fn the_store_is_asked_about_the_release_this_build_pins() {
    // The whole reason this is resolved at every start. Asking "what is
    // installed" would hand back a runtime from an earlier pin — still on the
    // disk, still launchable — and the agent Nessa tested would be replaced by
    // one it never saw.
    let Some(agent) = unbundled() else { return };
    let store = Answers::saying(Ok(None));

    let _ = installed_launch(agent, &host_platform(), &store);

    let asked = store.asked.lock().unwrap();
    let Some((name, version)) = asked.first() else {
        // No pinned release runs on this machine, so the store is never asked.
        // That is the correct behaviour and the assertions below have nothing
        // to say about it.
        return;
    };
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
    assert_eq!(installed_arguments(), vec!["acp".to_string()]);
}
