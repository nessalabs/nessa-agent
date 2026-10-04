use super::*;
use crate::conversation::application::{
    CatalogueHead, CataloguePage, CatalogueReadError, CatalogueReadOperation, CatalogueReadSource,
    ConversationFuture, RecordReadLease,
};
use crate::conversation::infrastructure::NessaCatalogueReadSource;
use mpsc::{Receiver, RecvError, Sender, TryRecvError};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::read_scope::CatalogueReadScope;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use tokio::sync::Semaphore;

struct Metadata {
    calls: AtomicUsize,
    reset: AtomicBool,
    panic: AtomicBool,
    fail: AtomicBool,
    gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}
impl Metadata {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            reset: AtomicBool::new(false),
            panic: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            gate: Mutex::new(None),
        }
    }
}
impl ConversationCatalogue for Metadata {
    fn head(&self, _: &OrganizationId, _: &PrincipalId) -> ConversationFuture<'_, CatalogueHead> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some((entered, release)) = self.gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
            assert!(
                !self.panic.load(Ordering::SeqCst),
                "injected metadata panic"
            );
            if self.fail.load(Ordering::SeqCst) {
                return Err(ConversationError::Unavailable);
            }
            Ok(CatalogueHead {
                incarnation: if self.reset.load(Ordering::SeqCst) {
                    "reset"
                } else {
                    "incarnation"
                }
                .into(),
                revision: 5,
            })
        })
    }
    fn page(&self, _: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ConversationError::Unavailable)
        })
    }
    fn resolve(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: &str,
        _: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ConversationError::Unavailable)
        })
    }
}
fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "receiver".into(),
        action_id: "read".into(),
    }
}
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn discover(metadata: Arc<Metadata>) -> Result<(NessaCatalogueSource, u64), CatalogueWorkerError> {
    NessaCatalogueSource::discover(
        metadata,
        caller(),
        id("receiver"),
        id("origin"),
        id("epoch-7"),
    )
}

#[test]
fn discovery_captures_one_head_and_reset_refuses_expected_scope() {
    let metadata = Arc::new(Metadata::new());
    let (mut source, head) = discover(metadata.clone()).unwrap();
    assert_eq!(head, 5);
    assert_eq!(metadata.calls.load(Ordering::SeqCst), 1);
    let scope = source.scope().clone();
    assert_eq!(scope.incarnation().as_str(), "incarnation");
    metadata.reset.store(true, Ordering::SeqCst);
    assert_eq!(
        source.head(&scope),
        Err(CatalogueSourceError::IdentityChanged)
    );
    assert_eq!(source.finish(), Ok(()));
    let mut clone = source.clone();
    assert_eq!(clone.finish(), Ok(()));
    assert_eq!(clone.head(&scope), Err(CatalogueSourceError::Unavailable));
    assert_eq!(metadata.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn initialization_failures_join_and_preserve_source_or_worker_meaning() {
    let metadata = Arc::new(Metadata::new());
    metadata.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        discover(metadata),
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::Unavailable
        ))
    ));
    let metadata = Arc::new(Metadata::new());
    metadata.panic.store(true, Ordering::SeqCst);
    assert!(matches!(
        discover(metadata),
        Err(CatalogueWorkerError::WorkerPanicked)
    ));
    let result = NessaCatalogueSource::start_with_identity(
        Arc::new(Metadata::new()),
        caller(),
        SourceScope::Discover {
            receiver: id("receiver"),
            origin: id("origin"),
            epoch: id("epoch-7"),
        },
        spawn_worker,
        || panic!("runtime initialization panic"),
    );
    assert!(matches!(result, Err(CatalogueWorkerError::WorkerPanicked)));
}

#[test]
fn readiness_receiver_loss_joins_actual_worker_before_return() {
    let finished = Arc::new(AtomicBool::new(false));
    let completion = finished.clone();
    let result = NessaCatalogueSource::start_with_readiness(
        Arc::new(Metadata::new()),
        caller(),
        SourceScope::Discover {
            receiver: id("receiver"),
            origin: id("origin"),
            epoch: id("epoch-7"),
        },
        move |run| {
            Ok(thread::spawn(move || {
                run();
                completion.store(true, Ordering::SeqCst);
            }))
        },
        make_runtime,
        |ready| {
            drop(ready);
            Err(RecvError)
        },
    );
    assert!(matches!(
        result,
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::Unavailable
        ))
    ));
    assert!(finished.load(Ordering::SeqCst));
}

#[test]
fn explicit_drain_waits_for_gated_read_and_retains_unexpected_exit() {
    let metadata = Arc::new(Metadata::new());
    let (source, _) = discover(metadata.clone()).unwrap();
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    *metadata.gate.lock().unwrap() = Some((entered, wait));
    let mut reader = source.clone();
    let scope = source.scope().clone();
    let read = thread::spawn(move || reader.head(&scope));
    started.recv().unwrap();
    let drain = source.clone();
    let (done, completed) = mpsc::channel();
    let joining = thread::spawn(move || {
        let result = drain.finish();
        done.send(result).unwrap();
    });
    assert!(matches!(completed.try_recv(), Err(TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(read.join().unwrap(), Ok(5));
    assert_eq!(completed.recv().unwrap(), Ok(()));
    joining.join().unwrap();
    assert_eq!(source.finish(), Ok(()));
    let metadata = Arc::new(Metadata::new());
    let (mut source, _) = discover(metadata.clone()).unwrap();
    let scope = source.scope().clone();
    metadata.panic.store(true, Ordering::SeqCst);
    assert_eq!(source.head(&scope), Err(CatalogueSourceError::Unavailable));
    assert_eq!(source.finish(), Err(CatalogueWorkerError::WorkerPanicked));
    assert_eq!(
        source.clone().finish(),
        Err(CatalogueWorkerError::WorkerPanicked)
    );
}

#[tokio::test]
async fn production_catalogue_owner_retains_lease_after_cancel_and_late_initialization_panic_through_shared_drain(
) {
    let metadata = Arc::new(Metadata::new());
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    *metadata.gate.lock().unwrap() = Some((entered, wait));
    let source = Arc::new(NessaCatalogueReadSource::new(
        metadata.clone(),
        id("origin"),
    ));
    let capacity = Arc::new(Semaphore::new(1));
    let lease = RecordReadLease::new(capacity.clone().try_acquire_owned().unwrap());
    let read = {
        let source = source.clone();
        tokio::spawn(async move {
            source
                .read(
                    CatalogueReadScope {
                        receiver_id: "receiver".into(),
                        organization_id: caller().organization_id,
                        owner_id: caller().principal_id,
                        access_epoch: 7,
                    },
                    CatalogueReadOperation::Head,
                    lease,
                )
                .await
        })
    };
    tokio::task::spawn_blocking(move || started.recv().unwrap())
        .await
        .unwrap();
    read.abort();
    assert!(read
        .await
        .err()
        .expect("cancelled read waiter")
        .is_cancelled());
    assert_eq!(capacity.available_permits(), 0);
    let mut first = Box::pin(source.shutdown());
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(matches!(first.as_mut().poll(cx), Poll::Pending)))
            .await
    );
    drop(first);
    let mut second = Box::pin(source.shutdown());
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(matches!(second.as_mut().poll(cx), Poll::Pending)))
            .await
    );
    metadata.panic.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    assert_eq!(second.await, Err(CatalogueReadError::WorkerPanicked));
    assert_eq!(
        source.shutdown().await,
        Err(CatalogueReadError::WorkerPanicked)
    );
    assert_eq!(capacity.available_permits(), 1);
}

#[test]
fn published_pass_request_owner_refuses_before_metadata_and_valid_requests_reach_it() {
    let metadata = Arc::new(Metadata::new());
    let caller = caller();
    let scope = Scope::new(
        id("receiver"),
        id("origin"),
        conversation_catalogue_stream(&caller.organization_id, &caller.principal_id),
        id("incarnation"),
        conversation_catalogue_schema(),
        id("epoch-7"),
    );
    let mut source = NessaCatalogueSource::new(metadata.clone(), caller, scope.clone()).unwrap();
    let valid = CataloguePass {
        scope,
        completed: 0,
        boundary: 5,
        cursor: None,
        generation: 1,
    };
    let selected = id("00000000-0000-4000-8000-000000000001");
    let mut cases = vec![];
    let mut pass = valid.clone();
    pass.generation = 0;
    cases.push(pass);
    let mut pass = valid.clone();
    pass.boundary = 0;
    cases.push(pass);
    for creation in [0, 6] {
        let mut pass = valid.clone();
        pass.cursor = Some(EntryKey {
            creation,
            id: selected.clone(),
        });
        cases.push(pass);
    }
    for pass in cases {
        assert_eq!(
            source.manifest(&ManifestRequest {
                pass: pass.clone(),
                max_entries: 1
            }),
            Err(CatalogueSourceError::InvalidRequest)
        );
        assert_eq!(
            source.resolve(&pass, &selected, 1),
            Err(CatalogueSourceError::InvalidRequest)
        );
    }
    for max_entries in [0, MAX_CATALOGUE_ENTRIES + 1] {
        assert_eq!(
            source.manifest(&ManifestRequest {
                pass: valid.clone(),
                max_entries
            }),
            Err(CatalogueSourceError::InvalidRequest)
        );
    }
    assert_eq!(metadata.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        source.manifest(&ManifestRequest {
            pass: valid.clone(),
            max_entries: 1
        }),
        Err(CatalogueSourceError::Unavailable)
    );
    assert_eq!(
        source.resolve(&valid, &selected, 1),
        Err(CatalogueSourceError::Unavailable)
    );
    assert_eq!(metadata.calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.finish(), Ok(()));
}
