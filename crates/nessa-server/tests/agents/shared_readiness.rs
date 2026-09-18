//! What one probe at a time costs, and what a caller is told when the machine
//! will not answer. No real machine is asked here: every host is a stub whose
//! timing the test owns.

use super::*;
use crate::agents::application::ProbeFailure;
use crate::agents_test_support::WaitingAgentProbe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// What a host that answers the same way about every agent produces: one entry
/// per agent this server reports on, in listing order.
fn every_agent(readiness: Readiness) -> Vec<(AgentId, Readiness)> {
    AgentId::ALL
        .iter()
        .map(|agent| (*agent, readiness))
        .collect()
}

/// A reader over `probe` whose callers wait no longer than `deadline`.
fn reader(probe: Arc<dyn AgentProbe>, deadline: Duration) -> Arc<SharedAgentReadiness> {
    Arc::new(SharedAgentReadiness::with_deadline(probe, deadline))
}

#[tokio::test]
async fn a_caller_arriving_while_a_probe_runs_joins_it_instead_of_starting_another() {
    // The bound this type exists for. The question takes no parameters, so a
    // second caller has nothing of its own to ask; starting a second probe would
    // buy them nothing and cost this machine another blocking thread and another
    // subprocess.
    let probe = Arc::new(WaitingAgentProbe::default());
    let shared = reader(probe.clone(), Duration::from_secs(2));

    let first = tokio::spawn({
        let shared = shared.clone();
        async move { shared.read().await }
    });
    probe.started().await;

    // Polled long enough to have reached the point where it would start a probe
    // of its own, then dropped. It is still waiting on the first one.
    let second = tokio::time::timeout(Duration::from_millis(50), shared.read()).await;
    assert!(
        second.is_err(),
        "the second caller should still be waiting on the probe that is running"
    );
    assert_eq!(
        probe.runs(),
        1,
        "a caller arriving mid-probe must not start a second one"
    );

    probe.release();
    let answers = first.await.unwrap().unwrap();
    assert_eq!(answers, every_agent(Readiness::Ready));
    assert_eq!(probe.most_at_once(), 1);
}

#[tokio::test]
async fn a_machine_that_will_not_answer_is_left_rather_than_waited_on() {
    // A locked keychain used to mean a request that never finished. The caller
    // now stops waiting, and is told the machine did not answer — never that the
    // agent is missing or signed out.
    let probe = Arc::new(WaitingAgentProbe::default());
    let shared = reader(probe.clone(), Duration::from_millis(50));

    let began = Instant::now();
    let answer = shared.read().await;
    let waited = began.elapsed();

    assert_eq!(answer, Err(ReadingFailure::Undetermined));
    assert!(
        waited < Duration::from_secs(1),
        "the caller waited {waited:?}, which is not a deadline"
    );
    // Release before the runtime goes away, so the blocked thread is not what
    // this test's tear-down waits on.
    probe.release();
}

/// A host that is not installed the first time it is asked and is the second,
/// which is what a person installing the agent mid-setup looks like.
#[derive(Default)]
struct InstalledOnSecondAsk {
    /// Per agent, because one probe asks about every agent: counting asks
    /// across all of them would make the first probe's later agents look like
    /// a second probe.
    asked: std::sync::Mutex<std::collections::HashSet<AgentId>>,
}

impl AgentProbe for InstalledOnSecondAsk {
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        Ok(!self.asked.lock().unwrap().insert(agent))
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        Ok(true)
    }
}

#[tokio::test]
async fn an_answer_is_never_kept_for_a_caller_who_was_not_waiting_for_it() {
    // Sharing one probe between concurrent callers must not become a cache.
    // Setup's "check again" is for the person who installs the agent while this
    // server is running, and an answer kept from a moment ago would tell them
    // what was true before they did it.
    let probe = Arc::new(InstalledOnSecondAsk::default());
    let shared = reader(probe.clone(), Duration::from_secs(2));

    assert_eq!(
        shared.read().await,
        Ok(every_agent(Readiness::NotInstalled))
    );
    assert_eq!(
        shared.read().await,
        Ok(every_agent(Readiness::Ready)),
        "a later caller must be told what this machine says now"
    );
}

/// A host that comes apart on the blocking thread.
struct PanickingProbe;

impl AgentProbe for PanickingProbe {
    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        panic!("this machine came apart while being asked");
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        Ok(true)
    }
}

#[tokio::test]
async fn a_probe_that_comes_apart_is_a_different_failure_from_one_that_ran_long() {
    // One is this server failing and one is this server declining; the route
    // says different things about them, so they must arrive here as different
    // values rather than as the same absent answer.
    let shared = reader(Arc::new(PanickingProbe), Duration::from_secs(2));
    assert_eq!(shared.read().await, Err(ReadingFailure::Lost));
    // And it does not wedge the route: the next caller is asked afresh.
    assert_eq!(shared.read().await, Err(ReadingFailure::Lost));
}
