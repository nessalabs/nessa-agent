//! A conversation placed on any worker of the pool: placed only while some
//! worker serves, bound to the least loaded one when its agent is opened,
//! kept there while that one serves, moved with nothing of its provider
//! context when it drains or leaves, and forgotten with the conversation.
use super::*;
use crate::conversation::infrastructure::{in_process_environment, FileBindings, FilePlacements};
use std::sync::Mutex;

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION).unwrap()
}

fn worker(byte: u8) -> PeerEnvironment {
    PeerEnvironment::new([byte; PeerEnvironment::KEY_BYTES])
}

fn context(name: &str) -> ExecutionSessionId {
    ExecutionSessionId::new(name).unwrap()
}

/// Paired environments whose connections carry what a test sets, each
/// with an environment of its own.
#[derive(Default)]
struct Workers {
    environments: Mutex<BTreeMap<PeerEnvironment, Arc<dyn Environment>>>,
    serving: Mutex<Vec<(PeerEnvironment, WorkerLoad)>>,
}

impl Workers {
    fn set(&self, serving: &[(PeerEnvironment, usize, bool)]) {
        *self.serving.lock().unwrap() = serving
            .iter()
            .map(|(peer, leases, draining)| {
                (
                    *peer,
                    WorkerLoad {
                        leases: *leases,
                        draining: *draining,
                    },
                )
            })
            .collect();
    }
}

impl PairedEnvironments for Workers {
    fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment> {
        self.environments
            .lock()
            .unwrap()
            .entry(*peer)
            .or_insert_with(in_process_environment)
            .clone()
    }

    fn connected(&self) -> Vec<PeerEnvironment> {
        self.serving
            .lock()
            .unwrap()
            .iter()
            .map(|(peer, _)| *peer)
            .collect()
    }

    fn serving(&self) -> Vec<(PeerEnvironment, WorkerLoad)> {
        self.serving.lock().unwrap().clone()
    }
}

struct Pool {
    _root: tempfile::TempDir,
    workers: Arc<Workers>,
    bindings: Arc<FileBindings>,
    environments: Environments,
}

fn pool() -> Pool {
    let root = tempfile::tempdir().unwrap();
    let workers = Arc::new(Workers::default());
    let bindings = Arc::new(FileBindings::new(root.path().join("bindings")).unwrap());
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlacements::new(root.path().join("placements")).unwrap()),
    )
    .with_peers(workers.clone(), bindings.clone());
    Pool {
        _root: root,
        workers,
        bindings,
        environments,
    }
}

impl Pool {
    async fn placed(&self) {
        self.environments
            .place(&conversation(), Some(POOL))
            .await
            .unwrap();
    }

    async fn lease(&self, avoid: &[PeerEnvironment]) -> Result<LeasePlace, ConversationError> {
        self.environments.for_lease(&conversation(), avoid).await
    }

    async fn bound(&self) -> Option<WorkerBinding> {
        self.bindings.binding(&conversation()).await.unwrap()
    }

    fn is(&self, place: &LeasePlace, peer: PeerEnvironment) -> bool {
        Arc::ptr_eq(&place.environment, &self.workers.environment(&peer))
    }
}

#[tokio::test]
async fn the_pool_is_placed_on_only_while_a_worker_serves_and_is_not_draining() {
    let pool = pool();
    for serving in [&[][..], &[(worker(1), 0, true), (worker(2), 3, true)][..]] {
        pool.workers.set(serving);
        assert!(!pool.environments.pool_serves());
        assert!(matches!(
            pool.environments.place(&conversation(), Some(POOL)).await,
            Err(ConversationError::EnvironmentNotConfigured)
        ));
    }
    pool.workers
        .set(&[(worker(1), 0, true), (worker(2), 3, false)]);
    assert!(pool.environments.pool_serves());
    pool.placed().await;
    assert_eq!(
        pool.environments.placement(&conversation()).await.unwrap(),
        Some(POOL.to_owned())
    );
    assert!(pool.environments.named(POOL).is_none());
}

#[cfg(unix)]
/// A cloud account connected or not, for [`Environments::with_workspaces`].
struct Cloud(bool);
#[cfg(unix)]
impl crate::conversation::application::ThreadEnvironments for Cloud {
    fn environment(
        &self,
        _: &ConversationId,
    ) -> Result<Arc<dyn crate::conversation::application::Environment>, ConversationError> {
        Ok(in_process_environment())
    }
    fn rest<'a>(
        &'a self,
        _: &'a ConversationId,
        _: nessa_sdk::domain::agent_execution::leases::LeaseEndCause,
    ) -> crate::conversation::application::EnvironmentFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn available(&self) -> crate::conversation::application::EnvironmentFuture<'_, bool> {
        let connected = self.0;
        Box::pin(async move { connected })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn with_no_paired_computer_online_the_pool_runs_in_the_cloud_when_it_can() {
    for (connected, asked) in [(true, POOL.to_owned()), (false, POOL.to_owned())] {
        let root = tempfile::tempdir().unwrap();
        let workers = Arc::new(Workers::default());
        let environments = Environments::new(
            in_process_environment(),
            BTreeMap::new(),
            Arc::new(FilePlacements::new(root.path().join("placements")).unwrap()),
        )
        .with_peers(
            workers.clone(),
            Arc::new(FileBindings::new(root.path().join("bindings")).unwrap()),
        )
        .with_workspaces(Arc::new(Cloud(connected)));
        let placed = environments.place(&conversation(), Some(&asked)).await;
        if connected {
            placed.unwrap();
            assert_eq!(
                environments
                    .placement(&conversation())
                    .await
                    .unwrap()
                    .as_deref(),
                Some(crate::conversation::application::placement::BOAT),
                "{asked}"
            );
        } else {
            // No cloud account: refused, as before.
            assert!(matches!(
                placed,
                Err(ConversationError::EnvironmentNotConfigured)
            ));
        }
    }
    // One computer asked for by name, offline, stays refused: the person
    // chose where it runs.
    let root = tempfile::tempdir().unwrap();
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlacements::new(root.path().join("placements")).unwrap()),
    )
    .with_peers(
        Arc::new(Workers::default()),
        Arc::new(FileBindings::new(root.path().join("bindings")).unwrap()),
    )
    .with_workspaces(Arc::new(Cloud(true)));
    assert!(matches!(
        environments
            .place(&conversation(), Some(&worker(1).name()))
            .await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
    // A host the configuration does not name is never sent to the cloud.
    let root = tempfile::tempdir().unwrap();
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlacements::new(root.path().join("placements")).unwrap()),
    )
    .with_workspaces(Arc::new(Cloud(true)));
    assert!(matches!(
        environments.place(&conversation(), Some("devbox")).await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
}

#[tokio::test]
async fn a_gateway_that_pairs_no_environments_places_nothing_on_the_pool() {
    let root = tempfile::tempdir().unwrap();
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlacements::new(root.path().join("placements")).unwrap()),
    );
    assert!(matches!(
        environments.place(&conversation(), Some(POOL)).await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
}

#[tokio::test]
async fn the_first_lease_binds_the_worker_with_fewest_leases_the_lowest_key_among_equals() {
    let pool = pool();
    pool.workers.set(&[
        (worker(3), 1, false),
        (worker(2), 1, false),
        (worker(1), 0, true),
    ]);
    pool.placed().await;
    assert_eq!(pool.bound().await, None);
    assert!(matches!(
        pool.environments.of(&conversation()).await,
        Err(ConversationError::Unavailable)
    ));

    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(2)));
    let context = place
        .context
        .expect("a pool lease carries its worker's context");
    assert_eq!(context.worker(), worker(2));
    assert_eq!(
        pool.bound().await,
        Some(WorkerBinding {
            worker: worker(2),
            context: None,
        })
    );
    let of = pool.environments.of(&conversation()).await.unwrap();
    assert!(Arc::ptr_eq(&of, &pool.workers.environment(&worker(2))));
}

#[tokio::test]
async fn a_bound_conversation_stays_on_its_worker_while_it_serves_however_loaded() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    let first = pool.lease(&[]).await.unwrap();
    first
        .context
        .unwrap()
        .opened(&context("opened-on-one"))
        .await;
    let written = WorkerBinding {
        worker: worker(1),
        context: Some(context("opened-on-one")),
    };
    // Written before the opening answers.
    assert_eq!(pool.bound().await.as_ref(), Some(&written));

    pool.workers
        .set(&[(worker(1), 7, false), (worker(2), 0, false)]);
    let again = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&again, worker(1)));
    let context_there = again.context.unwrap();
    assert!(context_there.holds(&context("opened-on-one")));
    assert!(!context_there.holds(&context("opened-elsewhere")));
    assert_eq!(pool.bound().await, Some(written));
}

#[tokio::test]
async fn a_conversation_whose_worker_drains_or_leaves_moves_with_no_provider_context() {
    for gone in [
        vec![(worker(1), 0, true), (worker(2), 4, false)],
        vec![(worker(2), 4, false)],
    ] {
        let pool = pool();
        pool.workers
            .set(&[(worker(1), 0, false), (worker(2), 0, false)]);
        pool.placed().await;
        let first = pool.lease(&[]).await.unwrap();
        assert!(pool.is(&first, worker(1)));
        pool.bindings
            .bind(
                &conversation(),
                &WorkerBinding {
                    worker: worker(1),
                    context: Some(context("opened-on-one")),
                },
            )
            .await
            .unwrap();

        pool.workers.set(&gone);
        assert!(pool.environments.left_the_pool(&worker(1)));
        assert!(!pool.environments.left_the_pool(&worker(2)));
        let moved = pool.lease(&[]).await.unwrap();
        assert!(pool.is(&moved, worker(2)));
        // What the first worker opened is not on this one.
        assert!(!moved.context.unwrap().holds(&context("opened-on-one")));
        assert_eq!(
            pool.bound().await,
            Some(WorkerBinding {
                worker: worker(2),
                context: None,
            })
        );
    }
}

#[tokio::test]
async fn a_worker_the_opener_avoids_is_passed_over_even_when_bound() {
    let pool = pool();
    pool.workers
        .set(&[(worker(1), 0, false), (worker(2), 5, false)]);
    pool.placed().await;
    assert!(pool.is(&pool.lease(&[]).await.unwrap(), worker(1)));
    let moved = pool.lease(&[worker(1)]).await.unwrap();
    assert!(pool.is(&moved, worker(2)));
    assert_eq!(pool.bound().await.unwrap().worker, worker(2));
}

#[tokio::test]
async fn with_no_worker_to_move_to_the_lease_is_asked_where_it_was_bound() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    pool.lease(&[])
        .await
        .unwrap()
        .context
        .unwrap()
        .opened(&context("opened-on-one"))
        .await;
    assert!(pool.bound().await.and_then(|bound| bound.context).is_some());

    pool.workers.set(&[]);
    let waiting = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&waiting, worker(1)));
    // Still that worker's, so what it opened is still restored there.
    assert!(waiting.context.unwrap().holds(&context("opened-on-one")));
}

#[tokio::test]
async fn a_conversation_never_bound_with_no_worker_serving_is_unavailable() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    pool.workers.set(&[(worker(1), 0, true)]);
    assert!(matches!(
        pool.lease(&[]).await,
        Err(ConversationError::Unavailable)
    ));
    assert_eq!(pool.bound().await, None);
}

#[tokio::test]
async fn a_context_opened_again_is_not_written_again() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    let place = pool.lease(&[]).await.unwrap();
    let context_there = place.context.unwrap();
    context_there.opened(&context("opened-on-one")).await;
    assert!(pool.bound().await.and_then(|bound| bound.context).is_some());
    // Erased under it: an opening of the same context writes nothing.
    pool.bindings.erase(&conversation()).await.unwrap();
    context_there.opened(&context("opened-on-one")).await;
    assert_eq!(pool.bound().await, None);
}

#[tokio::test]
async fn a_conversation_off_the_pool_carries_no_worker_context() {
    let pool = pool();
    pool.environments
        .place(&conversation(), None)
        .await
        .unwrap();
    let place = pool.lease(&[]).await.unwrap();
    assert!(place.context.is_none());
    assert_eq!(pool.bound().await, None);
}

#[tokio::test]
async fn erasing_a_pool_conversation_forgets_its_worker_and_its_placement() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    pool.lease(&[]).await.unwrap();
    assert!(pool.bound().await.is_some());
    pool.environments.erase(&conversation()).await.unwrap();
    assert_eq!(pool.bound().await, None);
    assert_eq!(
        pool.environments.placement(&conversation()).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn leases_chosen_at_once_are_spread_before_any_reaches_its_worker() {
    let pool = pool();
    pool.workers
        .set(&[(worker(1), 0, false), (worker(2), 0, false)]);
    let other = ConversationId::new("6d1b0a3f-2e4c-4d8b-9f70-3c0e5b2d8f41").unwrap();
    pool.placed().await;
    pool.environments.place(&other, Some(POOL)).await.unwrap();
    // Neither lease has reached its worker's connection yet, and both are
    // chosen at once: each reads its binding before either chooses.
    let (first, second) = tokio::join!(pool.lease(&[]), pool.environments.for_lease(&other, &[]));
    let (first, second) = (first.unwrap(), second.unwrap());
    // Whichever chose first took the lowest key; the other, the next.
    assert!(
        (pool.is(&first, worker(1)) && pool.is(&second, worker(2)))
            || (pool.is(&first, worker(2)) && pool.is(&second, worker(1)))
    );
    // Let go once asked: the connection counts them from then on.
    drop((first, second));
    let third = ConversationId::new("7e2c1b4a-3f5d-4e9c-8a81-4d1f6c3e9a52").unwrap();
    pool.environments.place(&third, Some(POOL)).await.unwrap();
    let after = pool.environments.for_lease(&third, &[]).await.unwrap();
    assert!(pool.is(&after, worker(1)));
}

#[tokio::test]
async fn a_pool_conversation_is_named_by_its_worker_once_bound_and_is_never_this_machine() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    assert_eq!(
        pool.environments.runs_on(&conversation()).await.unwrap(),
        Some(POOL.to_owned())
    );
    assert!(!pool
        .environments
        .reads_this_machine(&conversation())
        .await
        .unwrap());
    pool.lease(&[]).await.unwrap();
    assert_eq!(
        pool.environments.runs_on(&conversation()).await.unwrap(),
        Some(worker(1).name())
    );
    assert!(!pool
        .environments
        .reads_this_machine(&conversation())
        .await
        .unwrap());
}
