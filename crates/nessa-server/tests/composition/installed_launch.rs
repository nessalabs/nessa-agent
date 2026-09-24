//! Installed launch resolution over a substitute runtime store.

use std::path::PathBuf;
use std::sync::Mutex;

use super::*;
use crate::agent_install::application::{
    Publication, PublishFailure, RuntimeStore, StagedArchive, StoreFailure,
};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, HostPlatform, PinnedRelease, ReleasePlatform,
};
use crate::agent_install::infrastructure::releases_for;
use crate::composition::desktop::bundled_launch;
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;

struct Answers {
    installed: Result<Option<PathBuf>, StoreFailure>,
    asked: Mutex<Vec<(String, String)>>,
}

impl Answers {
    fn saying(installed: Result<Option<PathBuf>, StoreFailure>) -> Self {
        Self {
            installed,
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl RuntimeStore for Answers {
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        self.installed.clone()
    }

    fn managed_launch(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<ExecutableUseSnapshot>, StoreFailure> {
        self.asked.lock().unwrap().push((
            agent.as_str().to_owned(),
            release.version().as_str().to_owned(),
        ));
        self.installed
            .clone()
            .map(|path| path.map(ExecutableUseSnapshot::unmanaged))
    }

    fn stage(&self, _agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        unreachable!("launch resolution does not stage downloads")
    }

    fn digest(&self, _staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        unreachable!("launch resolution does not hash archives")
    }

    fn publish(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
        _staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure> {
        unreachable!("launch resolution does not publish runtimes")
    }

    fn discard(&self, _staged: StagedArchive) {
        unreachable!("launch resolution stages nothing to discard")
    }
}

fn unbundled() -> Option<AgentId> {
    AgentId::ALL
        .iter()
        .copied()
        .find(|agent| bundled_launch(*agent).is_none())
}

fn served() -> Option<(AgentId, HostPlatform)> {
    let agent = unbundled()?;
    let name = AgentName::parse(agent.name()).expect("agent names are valid pin names");
    let release = releases_for(&name)
        .expect("compiled pins parse")
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
fn verified_current_pin_is_the_launch() {
    let Some((agent, host)) = served() else {
        return;
    };
    let executable = PathBuf::from("/managed/opencode");
    let store = Answers::saying(Ok(Some(executable.clone())));

    let InstalledLaunch::Ready(launch) = installed_launch(agent, &host, &store).unwrap() else {
        panic!("verified current pin should be ready")
    };
    assert_eq!(launch.executable(), executable);
}

#[test]
fn no_record_is_missing() {
    let Some((agent, host)) = served() else {
        return;
    };
    assert_eq!(
        installed_launch(agent, &host, &Answers::saying(Ok(None))).unwrap(),
        InstalledLaunch::Missing
    );
}

#[test]
fn unsupported_host_is_distinct_and_does_not_query_the_store() {
    let Some((agent, _)) = served() else { return };
    let host = HostPlatform::new(
        ReleasePlatform::new("plan9", "sparc64").unwrap(),
        None,
        false,
    );
    let store = Answers::saying(Ok(Some(PathBuf::from("/wrong-host/opencode"))));

    assert_eq!(
        installed_launch(agent, &host, &store).unwrap(),
        InstalledLaunch::UnsupportedHost
    );
    assert!(store.asked.lock().unwrap().is_empty());
}

#[test]
fn unreadable_record_is_unknown_instead_of_missing() {
    let Some((agent, host)) = served() else {
        return;
    };
    let failure = StoreFailure::Unreadable("permission denied".into());

    assert_eq!(
        installed_launch(agent, &host, &Answers::saying(Err(failure.clone()))).unwrap(),
        InstalledLaunch::Unknown(failure)
    );
}

#[test]
fn the_store_is_asked_about_the_current_preferred_release() {
    let Some((agent, host)) = served() else {
        return;
    };
    let store = Answers::saying(Ok(None));

    let _ = installed_launch(agent, &host, &store);

    let asked = store.asked.lock().unwrap();
    let (name, version) = asked.first().expect("served hosts query the store");
    assert_eq!(name, agent.name());
    assert!(!version.is_empty());
}

#[test]
fn installed_arguments_are_agent_specific() {
    assert_eq!(installed_arguments(AgentId::Opencode), ["acp"]);
    assert!(installed_arguments(AgentId::Claude).is_empty());
    assert!(installed_arguments(AgentId::Codex).is_empty());
}
