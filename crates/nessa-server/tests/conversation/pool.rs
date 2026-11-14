//! A conversation placed on any worker of the pool: placed only while some
//! worker serves, bound to the least loaded one when its agent is opened,
//! kept there while that one serves, moved with nothing of its provider
//! context when it drains or leaves, and forgotten with the conversation.
use super::*;
use crate::conversation::domain::Place;
use crate::conversation::infrastructure::{in_process_environment, FilePlaces};
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
    /// Processor use each machine last reported, in tenths of a percent.
    cpu: Mutex<BTreeMap<PeerEnvironment, u32>>,
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

    fn machine_use(&self, peer: &PeerEnvironment) -> Option<MachineUse> {
        self.cpu.lock().unwrap().get(peer).map(|cpu| MachineUse {
            cpu_permille: *cpu,
            memory_permille: 100,
        })
    }
}

struct Pool {
    _root: tempfile::TempDir,
    workers: Arc<Workers>,
    places: Arc<FilePlaces>,
    environments: Environments,
}

fn pool() -> Pool {
    let root = tempfile::tempdir().unwrap();
    let workers = Arc::new(Workers::default());
    let places = Arc::new(FilePlaces::new(root.path()).unwrap());
    let environments = Environments::new(in_process_environment(), BTreeMap::new(), places.clone())
        .with_peers(workers.clone());
    Pool {
        _root: root,
        workers,
        places,
        environments,
    }
}

impl Pool {
    async fn placed(&self) {
        self.environments
            .place(&conversation(), &Place::Pool, None)
            .await
            .unwrap();
    }

    async fn lease(&self, avoid: &[PeerEnvironment]) -> Result<LeasePlace, ConversationError> {
        self.environments
            .for_lease(&conversation(), avoid, None)
            .await
    }

    async fn serves(&self) -> bool {
        let settings = self.environments.pool_settings(None).await.unwrap();
        self.environments.pool_serves(&settings)
    }

    async fn bound(&self) -> Option<WorkerBinding> {
        self.places
            .read(&conversation())
            .await
            .unwrap()
            .worker()
            .cloned()
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
        assert!(!pool.serves().await);
        assert!(matches!(
            pool.environments
                .place(&conversation(), &Place::Pool, None)
                .await,
            Err(ConversationError::EnvironmentNotConfigured)
        ));
    }
    pool.workers
        .set(&[(worker(1), 0, true), (worker(2), 3, false)]);
    assert!(pool.serves().await);
    pool.placed().await;
    assert_eq!(
        pool.environments.placement(&conversation()).await.unwrap(),
        Place::Pool
    );
    assert!(pool.environments.named(&Place::Pool).is_none());
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
        _: crate::conversation::domain::RestCause,
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
    for (connected, asked) in [(true, Place::Pool), (false, Place::Pool)] {
        let root = tempfile::tempdir().unwrap();
        let workers = Arc::new(Workers::default());
        let environments = Environments::new(
            in_process_environment(),
            BTreeMap::new(),
            Arc::new(FilePlaces::new(root.path()).unwrap()),
        )
        .with_peers(workers.clone())
        .with_workspaces(Arc::new(Cloud(connected)));
        let placed = environments.place(&conversation(), &asked, None).await;
        if connected {
            placed.unwrap();
            assert_eq!(
                environments.placement(&conversation()).await.unwrap(),
                Place::CloudVm,
                "{asked:?}"
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
        Arc::new(FilePlaces::new(root.path()).unwrap()),
    )
    .with_peers(Arc::new(Workers::default()))
    .with_workspaces(Arc::new(Cloud(true)));
    assert!(matches!(
        environments
            .place(&conversation(), &Place::Peer(worker(1)), None)
            .await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
    // A host the configuration does not name is never sent to the cloud.
    let root = tempfile::tempdir().unwrap();
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlaces::new(root.path()).unwrap()),
    )
    .with_workspaces(Arc::new(Cloud(true)));
    assert!(matches!(
        environments
            .place(&conversation(), &Place::named("devbox").unwrap(), None)
            .await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
}

/// Boat asked for by name with no Boat account connected is refused as not
/// configured, at placement and, for a chat placed there before the account
/// went away, at its next lease: never as a host that cannot be reached.
#[cfg(unix)]
#[tokio::test]
async fn boat_without_a_connected_account_is_not_configured() {
    let root = tempfile::tempdir().unwrap();
    let with = |connected| {
        Environments::new(
            in_process_environment(),
            BTreeMap::new(),
            Arc::new(FilePlaces::new(root.path()).unwrap()),
        )
        .with_workspaces(Arc::new(Cloud(connected)))
    };
    assert!(matches!(
        with(false)
            .place(&conversation(), &Place::CloudVm, None)
            .await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
    with(true)
        .place(&conversation(), &Place::CloudVm, None)
        .await
        .unwrap();
    assert!(with(true)
        .for_lease(&conversation(), &[], None)
        .await
        .is_ok());
    assert!(matches!(
        with(false).for_lease(&conversation(), &[], None).await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
}

#[tokio::test]
async fn a_gateway_that_pairs_no_environments_places_nothing_on_the_pool() {
    let root = tempfile::tempdir().unwrap();
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlaces::new(root.path()).unwrap()),
    );
    assert!(matches!(
        environments
            .place(&conversation(), &Place::Pool, None)
            .await,
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
        pool.places
            .write(
                &conversation(),
                &PlaceRecord::in_pool(WorkerBinding {
                    worker: worker(1),
                    context: Some(context("opened-on-one")),
                }),
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
    pool.places.erase(&conversation()).await.unwrap();
    context_there.opened(&context("opened-on-one")).await;
    assert_eq!(pool.bound().await, None);
}

#[tokio::test]
async fn a_conversation_off_the_pool_carries_no_worker_context() {
    let pool = pool();
    pool.environments
        .place(&conversation(), &Place::Here, None)
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
        Place::Here
    );
}

#[tokio::test]
async fn leases_chosen_at_once_are_spread_before_any_reaches_its_worker() {
    let pool = pool();
    pool.workers
        .set(&[(worker(1), 0, false), (worker(2), 0, false)]);
    let other = ConversationId::new("6d1b0a3f-2e4c-4d8b-9f70-3c0e5b2d8f41").unwrap();
    pool.placed().await;
    pool.environments
        .place(&other, &Place::Pool, None)
        .await
        .unwrap();
    // Neither lease has reached its worker's connection yet, and both are
    // chosen at once: each reads its binding before either chooses.
    let (first, second) = tokio::join!(
        pool.lease(&[]),
        pool.environments.for_lease(&other, &[], None)
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    // Whichever chose first took the lowest key; the other, the next.
    assert!(
        (pool.is(&first, worker(1)) && pool.is(&second, worker(2)))
            || (pool.is(&first, worker(2)) && pool.is(&second, worker(1)))
    );
    // Let go once asked: the connection counts them from then on.
    drop((first, second));
    let third = ConversationId::new("7e2c1b4a-3f5d-4e9c-8a81-4d1f6c3e9a52").unwrap();
    pool.environments
        .place(&third, &Place::Pool, None)
        .await
        .unwrap();
    let after = pool
        .environments
        .for_lease(&third, &[], None)
        .await
        .unwrap();
    assert!(pool.is(&after, worker(1)));
}

#[tokio::test]
async fn a_pool_conversation_is_named_by_its_worker_once_bound_and_is_never_this_machine() {
    let pool = pool();
    pool.workers.set(&[(worker(1), 0, false)]);
    pool.placed().await;
    assert_eq!(
        pool.environments.runs_on(&conversation()).await.unwrap(),
        Place::Pool
    );
    assert!(!pool
        .environments
        .reads_this_machine(&conversation())
        .await
        .unwrap());
    pool.lease(&[]).await.unwrap();
    assert_eq!(
        pool.environments.runs_on(&conversation()).await.unwrap(),
        Place::Peer(worker(1))
    );
    assert!(!pool
        .environments
        .reads_this_machine(&conversation())
        .await
        .unwrap());
}

/// Pool preferences a test sets: the same answer for every scope.
struct Preferences(Mutex<crate::conversation::domain::PoolSettings>);

impl Preferences {
    fn new(order: &[PeerEnvironment], excluded: &[PeerEnvironment], busy: u32) -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            crate::conversation::domain::PoolSettings::new(
                crate::conversation::domain::SettingsScope::Global,
                crate::conversation::domain::PoolPreference::new(order.to_vec(), excluded.to_vec())
                    .unwrap(),
                None,
                busy,
                busy,
            ),
        )))
    }
}

#[async_trait::async_trait]
impl crate::conversation::application::PoolSettingsStore for Preferences {
    async fn read(
        &self,
        _: &crate::conversation::domain::SettingsScope,
    ) -> Result<
        crate::conversation::domain::PoolSettings,
        crate::conversation::application::PoolSettingsError,
    > {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save(
        &self,
        _: crate::conversation::domain::SettingsScope,
        _: Option<crate::conversation::domain::PoolPreference>,
        _: crate::conversation::application::PoolCaller,
    ) -> Result<
        crate::conversation::domain::PoolSettings,
        crate::conversation::application::PoolSettingsError,
    > {
        unreachable!("placement never saves")
    }
}

fn preferred(order: &[PeerEnvironment], excluded: &[PeerEnvironment], busy: u32) -> Pool {
    let Pool {
        _root,
        workers,
        places,
        environments,
    } = pool();
    Pool {
        _root,
        workers,
        places,
        environments: environments.with_pool_settings(Preferences::new(order, excluded, busy)),
    }
}

impl Workers {
    /// Each worker serving, with its leases and its machine's processor
    /// use in tenths of a percent.
    fn set_busy(&self, serving: &[(PeerEnvironment, usize, u32)]) {
        self.set(
            &serving
                .iter()
                .map(|(peer, leases, _)| (*peer, *leases, false))
                .collect::<Vec<_>>(),
        );
        *self.cpu.lock().unwrap() = serving.iter().map(|(peer, _, cpu)| (*peer, *cpu)).collect();
    }
}

#[tokio::test]
async fn the_first_computer_in_the_order_takes_new_work_however_many_leases_it_holds() {
    let pool = preferred(&[worker(3), worker(1)], &[], 900);
    pool.workers
        .set_busy(&[(worker(1), 0, 10), (worker(2), 0, 10), (worker(3), 5, 10)]);
    pool.placed().await;
    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(3)));
}

#[tokio::test]
async fn a_busy_computer_is_passed_over_for_the_next_in_order_and_kept_when_all_are_busy() {
    let pool = preferred(&[worker(3), worker(1)], &[], 900);
    pool.workers
        .set_busy(&[(worker(1), 0, 10), (worker(2), 0, 10), (worker(3), 0, 950)]);
    pool.placed().await;
    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(1)), "the second in order, not busy");
    drop(place);
    // Bound, it stays while it may take the conversation, busy or not.
    pool.workers
        .set_busy(&[(worker(1), 0, 990), (worker(2), 0, 10), (worker(3), 0, 10)]);
    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(1)));
    drop(place);
    // A new conversation with every computer busy goes to the first in order.
    let other = ConversationId::new("6d1b0a3f-2e4c-4d8b-9f70-3cae5b2d8f41").unwrap();
    pool.workers.set_busy(&[
        (worker(1), 0, 990),
        (worker(2), 0, 990),
        (worker(3), 0, 990),
    ]);
    pool.environments
        .place(&other, &Place::Pool, None)
        .await
        .unwrap();
    let place = pool
        .environments
        .for_lease(&other, &[], None)
        .await
        .unwrap();
    assert!(pool.is(&place, worker(3)));
}

#[tokio::test]
async fn a_computer_left_out_is_never_chosen_and_a_bound_one_moves_off_it() {
    let pool = preferred(&[], &[worker(1)], 900);
    pool.workers.set_busy(&[(worker(1), 0, 10)]);
    // Only a left-out computer serves: the pool cannot be placed on.
    assert!(!pool.serves().await);
    assert!(matches!(
        pool.environments
            .place(&conversation(), &Place::Pool, None)
            .await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
    pool.workers
        .set_busy(&[(worker(1), 0, 10), (worker(2), 3, 10)]);
    pool.placed().await;
    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(2)));
    drop(place);
    // A binding to a computer that was later left out moves off it.
    pool.places
        .write(
            &conversation(),
            &PlaceRecord::in_pool(WorkerBinding {
                worker: worker(1),
                context: None,
            }),
        )
        .await
        .unwrap();
    let place = pool.lease(&[]).await.unwrap();
    assert!(pool.is(&place, worker(2)));
}

#[cfg(unix)]
#[tokio::test]
async fn with_every_computer_busy_a_new_pool_conversation_overflows_to_the_cloud() {
    let root = tempfile::tempdir().unwrap();
    let workers = Arc::new(Workers::default());
    let environments = Environments::new(
        in_process_environment(),
        BTreeMap::new(),
        Arc::new(FilePlaces::new(root.path()).unwrap()),
    )
    .with_peers(workers.clone())
    .with_workspaces(Arc::new(Cloud(true)))
    .with_pool_settings(Preferences::new(&[], &[], 800));
    workers.set_busy(&[(worker(1), 0, 850), (worker(2), 0, 800)]);
    environments
        .place(&conversation(), &Place::Pool, None)
        .await
        .unwrap();
    assert_eq!(
        environments.placement(&conversation()).await.unwrap(),
        Place::CloudVm
    );
    // One computer below its limit keeps new work on the pool.
    let other = ConversationId::new("6d1b0a3f-2e4c-4d8b-9f70-3cae5b2d8f41").unwrap();
    workers.set_busy(&[(worker(1), 0, 850), (worker(2), 0, 799)]);
    environments
        .place(&other, &Place::Pool, None)
        .await
        .unwrap();
    assert_eq!(environments.placement(&other).await.unwrap(), Place::Pool);
}

#[tokio::test]
async fn the_pool_standing_names_each_computers_load_and_place() {
    let pool = preferred(&[worker(3)], &[worker(2)], 900);
    pool.workers
        .set_busy(&[(worker(1), 2, 950), (worker(2), 0, 10), (worker(3), 1, 10)]);
    let settings = pool.environments.pool_settings(None).await.unwrap();
    let standing = pool.environments.pool_standing(&settings);
    let first = &standing[&worker(3).name()];
    assert_eq!(
        (first.rank, first.agents, first.busy),
        (Some(0), Some(1), false)
    );
    let unordered = &standing[&worker(1).name()];
    assert_eq!(
        (unordered.rank, unordered.agents, unordered.busy),
        (Some(1), Some(2), true)
    );
    assert_eq!(unordered.machine.map(|used| used.cpu_permille), Some(950));
    assert_eq!(standing[&worker(2).name()].rank, None, "left out");
}
