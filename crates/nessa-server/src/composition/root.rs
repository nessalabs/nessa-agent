use crate::cli::entrypoint::{Command, LocalProvisioning, HELP};
use crate::conversation::application::{CatalogueReadError, ConversationError, RecordReadError};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::desktop_runtime::{
    application::{restore_retirement, retire},
    infrastructure::{ConversationDirectory, RetirementFiles},
};
use crate::env::Environment;
use crate::product::{ProductRouteState, WatchTaskFault};
use crate::server::entrypoint::http;
use crate::{
    app::dependencies::RuntimeDependencies,
    core::{
        Launch, NativeFailure, NativeShutdownFailure, PassiveReaderOutcomes,
        PassiveReaderShutdownFailure, RunError, ShutdownFailure, WatchDrainOutcome,
        WatchShutdownFailure,
    },
    env::UptimeBackend,
};
use axum::{serve::Listener, Extension, Router};
use nessa_gateway_endpoint::{
    application::PublishGatewayEndpoint,
    domain::{
        EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement, ManagedRuntimeIdentity,
    },
    infrastructure::FileEndpointPublication,
};
use std::future::Future;
use std::io::{Error, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio::{net::TcpStream, task::JoinError};
use uuid::Uuid;

pub struct CompositionRoot;

impl CompositionRoot {
    /// Dispatch the single CLI contract; local provisioning never contacts a server.
    ///
    /// `launch` is what started this process, resolved once at the edge. Only a
    /// launch the desktop host registered may touch that registration's
    /// recovery record, so it travels with the command rather than being
    /// re-derived from the environment wherever it is needed.
    pub async fn run(command: Command, launch: &Launch) -> Result<(), RunError> {
        match command {
            Command::Help => {
                std::io::stdout().write_all(HELP.as_bytes())?;
                Ok(())
            }
            Command::Server(provisioning) => Self::serve(provisioning, launch).await,
            Command::Desktop(directory) => Self::serve_desktop(&directory, launch).await,
            Command::Offline(mut args) => {
                if args.get(1).is_some_and(|v| v == "init")
                    && !args.iter().any(|v| v == "--owner-token-file")
                {
                    let directory = Environment::auth_directory_from_system()?.join("surfaces");
                    nessa_local_storage::create_directory(&directory)
                        .map_err(|e| RunError::Authentication(e.to_string()))?;
                    args.splice(
                        2..2,
                        [
                            "--owner-token-file".into(),
                            directory
                                .join("nessa-cli.token")
                                .to_string_lossy()
                                .into_owned(),
                        ],
                    );
                }
                super::auth_command::execute(&args)
            }
            Command::Token {
                credential_file,
                ttl_seconds,
            } => super::cli::online(true, credential_file, ttl_seconds),
            Command::Doctor { credential_file } => super::cli::online(false, credential_file, None),
            Command::InstallAgent { agent } => super::install_command::execute(&agent).await,
            #[cfg(unix)]
            Command::McpRelay {
                socket,
                server,
                configuration,
            } => {
                let ended =
                    crate::mcp_servers::infrastructure::run(&socket, &server, &configuration).await;
                // Exits here rather than returning: the harness's stdin is read
                // on a blocking thread nothing can cancel, and the runtime
                // would wait for it on the way out — leaving a relay whose
                // server has ended alive for as long as the harness keeps its
                // stdin open. Its stdout was flushed when the relay ended.
                let status = match ended {
                    Ok(()) => 0,
                    Err(failure) => {
                        tracing::error!(%failure, "MCP stand-in ended without serving");
                        1
                    }
                };
                std::process::exit(status)
            }
            #[cfg(not(unix))]
            Command::McpRelay { .. } => {
                Err(RunError::Agent("MCP stand-ins require Unix sockets".into()))
            }
        }
    }

    pub async fn serve(provisioning: LocalProvisioning, launch: &Launch) -> Result<(), RunError> {
        Self::serve_runtime(None, provisioning, launch).await
    }

    /// The packaged app owns its private namespace and has no operator to run the
    /// offline commands, so it always provisions what is missing.
    async fn serve_desktop(bundle: &std::path::Path, launch: &Launch) -> Result<(), RunError> {
        Self::serve_runtime(Some(bundle), LocalProvisioning::Automatic, launch).await
    }

    async fn serve_runtime(
        bundle: Option<&std::path::Path>,
        provisioning: LocalProvisioning,
        launch: &Launch,
    ) -> Result<(), RunError> {
        // A managed launch supersedes whatever its own registration last wrote
        // down about giving up. Nothing else may: a `nessa server` someone runs
        // in the same data directory while diagnosing exactly this problem
        // would otherwise erase the record the desktop host is about to recover
        // the service by. One launchd service per label means no second process
        // can be holding this generation while this one starts.
        launch.forget_startup_failure();
        let config = Environment::from_system()?;
        if provisioning == LocalProvisioning::Automatic {
            super::provisioning::ensure_local_credentials(&config)?;
        }
        let dependencies = runtime_dependencies(&config);
        let super::local_auth::LocalProduct {
            routes: product,
            record_reader,
            catalogue_reader,
            warm_ups,
            mcp,
            native,
        } = super::local_auth::product_state(&config, dependencies.clock.clone(), bundle).await?;
        let conversations = product.conversations.clone();
        // Shared with the retirement below, which asks whether any of them may
        // still hold an agent process (ADR 221).
        let warm_ups = Arc::new(warm_ups);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let retirement_clock = product.clock.clone();
        let desktop_identity = if let Some(bundle) = bundle {
            let configured = std::env::var("NESSA_RUNTIME_FINGERPRINT")
                .map_err(|_| RunError::Runtime("missing desktop runtime fingerprint".into()))?;
            let generation = Environment::service_generation_from_system()
                .ok_or_else(|| RunError::Runtime("missing desktop service generation".into()))?;
            Some(super::desktop::runtime_identity(
                bundle,
                configured,
                generation,
                Uuid::new_v4().to_string(),
                std::process::id(),
            )?)
        } else {
            None
        };
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let retirement_files = if let Some(identity) = &desktop_identity {
            let root = config
                .auth_directory
                .as_ref()
                .and_then(|path| path.parent())
                .ok_or_else(|| RunError::Agent("missing desktop namespace".into()))?;
            let files = RetirementFiles::new(root, retirement_clock).map_err(RunError::Agent)?;
            let fence = files.fence().map_err(RunError::Agent)?;
            restore_retirement(fence.as_ref(), identity, conversations.as_deref())
                .await
                .map_err(RunError::Agent)?;
            Some(files)
        } else {
            None
        };
        let listen_addr = config.listen_addr();
        let listener = tokio::net::TcpListener::bind(&listen_addr)
            .await
            .map_err(|source| RunError::Bind {
                addr: listen_addr.clone(),
                source,
            })?;
        let bound_address = listener.local_addr().map_err(|error| {
            tracing::error!(
                operation = "read bound listener address",
                error.kind = ?error.kind(),
                error.raw_os_code = ?error.raw_os_error(),
                error = %error,
                "gateway listener inspection failed",
            );
            RunError::Serve(error)
        })?;
        // After the browser bind and before anything is advertised: a native
        // bind failure drops the browser listener unserved (design row S10).
        let native = match native {
            Some(prepared) => {
                let bound =
                    super::native_pairing::bind(prepared, dependencies.clock.clone()).await?;
                tracing::info!(
                    native_listen_addr = %bound.local_address(),
                    "native pairing listening"
                );
                Some(bound)
            }
            None => None,
        };
        let endpoint_identity = EndpointIdentity::new(
            desktop_identity
                .as_ref()
                .map(|identity| identity.instance().as_str().to_owned())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            std::process::id(),
        )
        .map_err(|error| RunError::Runtime(error.into()))?;
        let endpoint =
            GatewayEndpoint::new(format!("ws://{bound_address}"), endpoint_identity.clone())
                .map_err(|error| RunError::Runtime(error.into()))?;
        let managed = desktop_identity
            .as_ref()
            .map(|identity| {
                ManagedRuntimeIdentity::new(
                    identity.fingerprint().as_str().to_owned(),
                    identity.generation().as_str().to_owned(),
                    identity.instance().as_str().to_owned(),
                    identity.process_id(),
                )
            })
            .transpose()
            .map_err(|error| RunError::Runtime(error.into()))?;
        let (endpoint_root, endpoint_directory) = config
            .gateway_endpoint_storage()
            .ok_or_else(|| RunError::Runtime("missing endpoint publication namespace".into()))?;
        let publication = FileEndpointPublication::new(endpoint_root, endpoint_directory);
        let advertisement = GatewayEndpointAdvertisement::new(endpoint.clone(), managed)
            .map_err(|error| RunError::Runtime(error.into()))?;
        tokio::task::spawn_blocking(move || {
            PublishGatewayEndpoint::new(&publication).execute(&advertisement)
        })
        .await
        .map_err(|error| RunError::Serve(std::io::Error::other(error.to_string())))?
        .map_err(RunError::Serve)?;

        let shutdown_product = product.clone();
        let mut router = http::router(product).layer(Extension(endpoint_identity));
        if let Some(identity) = &desktop_identity {
            router = router.layer(Extension(identity.clone()));
        }

        tracing::info!(
            listen_addr = %bound_address,
            stage = config.stage.as_str(),
            "nessa server listening",
        );

        // Only now: the runtime's first launch is slow because the operating
        // system scans it, and that wait belongs here, with the window already
        // up, rather than inside the user's first message.
        for warm_up in warm_ups.iter() {
            warm_up.start();
        }
        // Stand-ins reach their servers through the relay from here on; each
        // opens a session, and a server process, of its own.
        #[cfg(unix)]
        let mcp_servers = mcp.map(|mcp| {
            tokio::spawn(mcp.relay.listen(mcp.listener));
            // Ends by itself once the last holder of the store lets go of it.
            tokio::spawn(
                crate::mcp_servers::infrastructure::ResourceTicketStore::sweep_periodically(
                    std::sync::Arc::downgrade(&mcp.resource_tickets),
                    super::mcp_servers::RESOURCE_TICKET_SWEEP,
                ),
            );
            (mcp.servers, mcp.ticket_recorder)
        });
        #[cfg(not(unix))]
        let _ = mcp;
        // Deletions a tombstone says did not finish — interrupted by the last
        // run's exit, or stopped short — are finished now, in the background:
        // a deletion that cannot finish is reported, never a reason to hold up
        // startup. Asking an agent about its own record of a session does not
        // wait for the warm-up just above; it spends its own launch budget.
        if let Some(service) = conversations.clone() {
            tokio::spawn(async move {
                match service.finish_deletions().await {
                    Ok(left) if left.unfinished.is_empty() && left.unreadable == 0 => {}
                    // A tombstone that could not be read is a deletion that
                    // cannot even be seen, so it is counted here too.
                    Ok(left) => tracing::warn!(
                        unfinished = left.unfinished.len(),
                        unreadable = left.unreadable,
                        "some deleted conversations are still not fully erased, or their tombstones could not be read"
                    ),
                    Err(error) => tracing::error!(
                        %error,
                        "deleted conversations could not be listed to finish their erasure"
                    ),
                }
            });
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if bundle.is_some() {
            if let Some(service) = conversations.clone() {
                let mut requests = signal(SignalKind::user_defined1()).map_err(RunError::Serve)?;
                tokio::spawn(async move {
                    while requests.recv().await.is_some() {
                        if let Err(error) = service.stop_active_agents().await {
                            tracing::error!(%error, "could not stop active agents");
                        }
                    }
                });
            }
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(identity) = desktop_identity {
            let files = retirement_files.expect("desktop files initialized before admission");
            let background = StartupWarmUps(warm_ups.clone());
            let conversation_data =
                ConversationDirectory::new(super::local_auth::conversation_root(
                    config
                        .auth_directory
                        .as_ref()
                        .and_then(|path| path.parent())
                        .ok_or_else(|| RunError::Agent("missing desktop namespace".into()))?,
                ));
            let service = conversations.clone();
            let mut requests = signal(SignalKind::user_defined2()).map_err(RunError::Serve)?;
            tokio::spawn(async move {
                while requests.recv().await.is_some() {
                    let request = match files.request() {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::error!(%error, "invalid desktop retirement request");
                            continue;
                        }
                    };
                    let result = retire(
                        request,
                        identity.clone(),
                        service.as_deref(),
                        &conversation_data,
                        &background,
                        &files,
                    )
                    .await;
                    if let Err(error) = files.result(&result) {
                        tracing::error!(%error, "could not acknowledge desktop retirement");
                    }
                }
            });
        }
        // An accept failure that ends the native listener, or its task ending
        // any other way, stops the whole gateway through the same path as a
        // process signal (design rows S14, S15).
        let (native_failure, native_failed) = tokio::sync::watch::channel(None);
        let mut native = native.map(|bound| super::native_pairing::start(bound, native_failure));
        let native_watch = native.as_ref().map(|_| native_failed.clone());
        // Admission stops independently of physical reader drain. The process
        // joins the cleanup owner and carries its retained report into its exit.
        // A panic before publication leaves the report unconfirmed.
        let report: Arc<Mutex<ShutdownReport>> = Arc::new(Mutex::new(ShutdownReport::Unreported));
        let slot = report.clone();
        let stop = stop_signal(shutdown_signal(), native_watch);
        let (served, cleanup) = serve_with_cleanup(listener, router, stop, async move {
            // Native peers are woken before anything else is waited on;
            // their drain is joined last, into the same report (row S12).
            if let Some(native) = native.as_mut() {
                native.signal_stop();
            }
            // Then watch admission closes before any physical cleanup is
            // polled (cleanup_product), and every outcome joins one report.
            cleanup_product(
                &slot,
                &shutdown_product,
                async {
                    match record_reader {
                        Some(reader) => reader.shutdown().await,
                        None => Ok(()),
                    }
                },
                async {
                    match catalogue_reader {
                        Some(reader) => reader.shutdown().await,
                        None => Ok(()),
                    }
                },
                conversations.map(|service| async move { service.shutdown().await }),
                async {
                    // After agents, whose stand-ins end with their servers.
                    #[cfg(unix)]
                    if let Some((servers, recorder)) = mcp_servers {
                        servers.stop().await;
                        // Last: the conversations' ends released their
                        // tickets, and each end is recorded before exit.
                        if let Some(recorder) = recorder {
                            recorder.finish().await;
                        }
                    }
                },
                async {
                    match native {
                        Some(native) => native.join().await,
                        None => Ok(()),
                    }
                },
                Duration::from_secs(30),
            )
            .await;
        })
        .await;
        let cleanup = cleanup.map_err(|error| {
            tracing::error!(%error, "gateway cleanup owner failed");
            RunError::Shutdown(None)
        });
        let native_failed = *native_failed.borrow();
        serve_outcome(served, native_failed, &report)?;
        cleanup
    }
}

/// Signal Axum before waiting for cleanup; retain and join the cleanup task.
async fn serve_with_cleanup(
    listener: impl Listener<Io = TcpStream, Addr = SocketAddr>,
    router: Router,
    signal: impl Future<Output = ()> + Send + 'static,
    cleanup: impl Future<Output = ()> + Send + 'static,
) -> (Result<(), Error>, Result<(), JoinError>) {
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let cleanup = tokio::spawn(async move {
        signal.await;
        let _ = stop.send(());
        cleanup.await;
    });
    let served = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = stopped.await;
        })
        .await;
    (served, cleanup.await)
}

/// This process's result, from serving, from the native listener, and from
/// what shutdown reported.
///
/// The report is read before either failure is propagated, so neither can
/// discard it. Axum 0.8's graceful-shutdown future always ends in `Ok(())`, so
/// `served` is vestigial today; it is taken and propagated rather than ignored
/// in case a later axum gives the serve loop an error path again. A native
/// listener failure is what stopped the gateway (design row S14), so it is the
/// result ahead of the report.
fn serve_outcome(
    served: std::io::Result<()>,
    native_failed: Option<std::io::ErrorKind>,
    report: &Mutex<ShutdownReport>,
) -> Result<(), RunError> {
    let unconfirmed = shutdown_result(report);
    served?;
    if let Some(kind) = native_failed {
        return Err(RunError::Native(NativeFailure::Listener(kind)));
    }
    unconfirmed
}

/// Resolves on a process signal, or when the native listener's channel changes:
/// a published failure (row S14), or its closing because the listener task
/// ended without being asked to, as on a panic (row S15). With native pairing
/// off there is no channel, and only the process signal stops the gateway.
async fn stop_signal(
    signal: impl Future<Output = ()>,
    native: Option<tokio::sync::watch::Receiver<Option<std::io::ErrorKind>>>,
) {
    let native_ended = async {
        match native {
            // Either outcome means the listener is no longer serving.
            Some(mut native) => {
                let _ = native.changed().await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        () = signal => {}
        () = native_ended => {}
    }
}

/// What the joined cleanup owner has established so far.
#[derive(Debug)]
enum ShutdownReport {
    /// Reader/conversation outcomes succeeded and MCP stop returned under its
    /// existing unit-returning contract; no separate MCP confirmation is inferred.
    Confirmed,
    /// Callback has not yet published reader evidence.
    Unreported,
    /// The report retains each result as it is observed, including deadline evidence.
    DrainsPending {
        readers: PassiveReaderOutcomes,
        watches: WatchDrainOutcome,
    },
    /// Both physical drains completed; conversation cleanup has not returned.
    ConversationsPending {
        readers: Result<(), PassiveReaderShutdownFailure>,
        watches: Result<(), WatchShutdownFailure>,
    },
    /// Reader and conversation outcomes are known; MCP stop has not returned.
    ServersPending {
        readers: Result<(), PassiveReaderShutdownFailure>,
        watches: Result<(), WatchShutdownFailure>,
        conversations: Result<(), ConversationError>,
    },
    /// MCP stop returned; native pairing's drain has not.
    NativePending {
        readers: Result<(), PassiveReaderShutdownFailure>,
        watches: Result<(), WatchShutdownFailure>,
        conversations: Result<(), ConversationError>,
    },
    /// All cleanup owners returned; at least one failed.
    Failed(ShutdownFailure),
}

/// Normal host consumer: close watch admission before polling physical cleanup.
/// ProductRouteState retains the original WatchOwners throughout this owned future.
// Each argument is one independently owned cleanup stage (watches, two
// readers, conversations, MCP, native) or the shared deadline; the report
// orders them, so they stay separate parameters.
#[allow(clippy::too_many_arguments)]
async fn cleanup_product(
    slot: &Mutex<ShutdownReport>,
    product: &ProductRouteState,
    record: impl Future<Output = Result<(), RecordReadError>>,
    catalogue: impl Future<Output = Result<(), CatalogueReadError>>,
    conversations: Option<impl Future<Output = Result<(), ConversationError>>>,
    servers: impl Future<Output = ()>,
    native: impl Future<Output = Result<(), NativeShutdownFailure>>,
    deadline: Duration,
) {
    product.close_watch_admission();
    passive_cleanup(
        slot,
        product.drain_watches(),
        record,
        catalogue,
        conversations,
        servers,
        native,
        deadline,
    )
    .await;
}

/// Own reader drain, conversation/MCP cleanup, and their report together.
/// Each pending stage publishes known evidence before its next await. The final
/// synchronous publication consumes that evidence rather than cloning errors.
// Each argument is one independently owned cleanup stage (watches, two
// readers, conversations, MCP, native) or the shared deadline; the report
// orders them, so they stay separate parameters.
#[allow(clippy::too_many_arguments)]
async fn passive_cleanup(
    slot: &Mutex<ShutdownReport>,
    watches: impl Future<Output = Result<(), WatchTaskFault>>,
    record: impl Future<Output = Result<(), RecordReadError>>,
    catalogue: impl Future<Output = Result<(), CatalogueReadError>>,
    conversations: Option<impl Future<Output = Result<(), ConversationError>>>,
    servers: impl Future<Output = ()>,
    native: impl Future<Output = Result<(), NativeShutdownFailure>>,
    deadline: Duration,
) {
    record_shutdown(
        slot,
        ShutdownReport::DrainsPending {
            readers: PassiveReaderOutcomes::default(),
            watches: WatchDrainOutcome::default(),
        },
    );
    tokio::pin!(record, catalogue, watches);
    let timeout = tokio::time::sleep(deadline);
    tokio::pin!(timeout);
    // One deadline covers every drain: it stays armed while any is pending and
    // fires once, recording evidence only against the drains still pending.
    let mut deadline_passed = false;
    loop {
        let (record_pending, catalogue_pending, watch_pending) = {
            let report = slot.lock().unwrap_or_else(PoisonError::into_inner);
            let ShutdownReport::DrainsPending {
                readers: outcomes,
                watches,
            } = &*report
            else {
                unreachable!()
            };
            (
                outcomes.record().is_none(),
                outcomes.catalogue().is_none(),
                !watches.complete(),
            )
        };
        let deadline_pending = !deadline_passed;
        if !record_pending && !catalogue_pending && !watch_pending {
            break;
        }
        // Ready completions win over a simultaneous deadline, including zero.
        // Each observed result is published synchronously before another await.
        tokio::select! {
            biased;
            result = &mut record, if record_pending => update_reader_report(slot, |outcomes| outcomes.observe_record(result)),
            result = &mut catalogue, if catalogue_pending => update_reader_report(slot, |outcomes| outcomes.observe_catalogue(result)),
            result = &mut watches, if watch_pending => {
                let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
                let ShutdownReport::DrainsPending { watches, .. } = &mut *report else { unreachable!() };
                watches.observe(result);
            },
            _ = &mut timeout, if deadline_pending => {
                deadline_passed = true;
                let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
                let ShutdownReport::DrainsPending { readers, watches } = &mut *report else { unreachable!() };
                if !readers.complete() {
                    readers.observe_deadline();
                    tracing::error!("passive reader shutdown exceeded deadline; retaining runtime until physical work ends");
                }
                if !watches.complete() {
                    watches.observe_deadline();
                    tracing::error!("watch shutdown exceeded deadline; retaining runtime until original watch tasks end");
                }
            }
        }
    }
    {
        let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let ShutdownReport::DrainsPending {
            readers: outcomes,
            watches,
        } = std::mem::replace(&mut *report, ShutdownReport::Unreported)
        else {
            unreachable!()
        };
        *report = ShutdownReport::ConversationsPending {
            readers: outcomes.into_result(),
            watches: watches.into_result(),
        };
    }
    let conversations = match conversations {
        Some(conversations) => conversations.await,
        None => Ok(()),
    };
    {
        let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let ShutdownReport::ConversationsPending { readers, watches } =
            std::mem::replace(&mut *report, ShutdownReport::Unreported)
        else {
            unreachable!()
        };
        *report = ShutdownReport::ServersPending {
            readers,
            watches,
            conversations,
        };
    }
    servers.await;
    {
        let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let ShutdownReport::ServersPending {
            readers,
            watches,
            conversations,
        } = std::mem::replace(&mut *report, ShutdownReport::Unreported)
        else {
            unreachable!()
        };
        *report = ShutdownReport::NativePending {
            readers,
            watches,
            conversations,
        };
    }
    let native = native.await;
    let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let ShutdownReport::NativePending {
        readers,
        watches,
        conversations,
    } = std::mem::replace(&mut *report, ShutdownReport::Unreported)
    else {
        unreachable!()
    };
    *report = match (watches, readers, conversations, native) {
        (watches, readers, conversations, Err(native)) => {
            ShutdownReport::Failed(ShutdownFailure::Native {
                readers,
                watches,
                conversations,
                native,
            })
        }
        (Err(watches), readers, conversations, Ok(())) => {
            ShutdownReport::Failed(ShutdownFailure::Watches {
                watches,
                readers,
                conversations,
            })
        }
        (Ok(()), Ok(()), Ok(()), Ok(())) => ShutdownReport::Confirmed,
        (Ok(()), Err(readers), Ok(()), Ok(())) => {
            ShutdownReport::Failed(ShutdownFailure::Readers(readers))
        }
        (Ok(()), Ok(()), Err(conversations), Ok(())) => {
            ShutdownReport::Failed(ShutdownFailure::Conversations(conversations))
        }
        (Ok(()), Err(readers), Err(conversations), Ok(())) => {
            ShutdownReport::Failed(ShutdownFailure::Both {
                readers,
                conversations,
            })
        }
    };
    if let ShutdownReport::Failed(error) = &*report {
        tracing::error!(%error, "gateway shutdown did not confirm all cleanup");
    }
}

fn update_reader_report(
    slot: &Mutex<ShutdownReport>,
    update: impl FnOnce(&mut PassiveReaderOutcomes),
) {
    let mut report = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let ShutdownReport::DrainsPending {
        readers: outcomes, ..
    } = &mut *report
    else {
        unreachable!()
    };
    update(outcomes);
}

fn record_shutdown(slot: &Mutex<ShutdownReport>, report: ShutdownReport) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = report;
}

/// Turn what graceful shutdown reported into this process's result.
///
/// Serving finishing is not the same fact as conversations confirming their
/// cleanup and audit delivery. A poisoned slot is read through rather than
/// discarded: the value is still whatever was last written, and throwing away a
/// recorded failure to report "never reported" would be a worse answer than the
/// one it replaced.
///
/// Taking a failure leaves `Unreported` behind, not `Confirmed`. Only one read
/// happens today, but a second one must not be able to call a run confirmed on
/// the strength of having already reported that it was not — nor invent a
/// failure for one that was confirmed, which is why `Confirmed` is left alone.
fn shutdown_result(report: &Mutex<ShutdownReport>) -> Result<(), RunError> {
    let mut slot = report.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(*slot, ShutdownReport::Confirmed) {
        return Ok(());
    }
    let unconfirmed = match std::mem::replace(&mut *slot, ShutdownReport::Unreported) {
        ShutdownReport::Failed(error) => Some(error),
        ShutdownReport::DrainsPending {
            readers: outcomes,
            watches,
        } => Some(ShutdownFailure::DrainsUnreported { outcomes, watches }),
        ShutdownReport::ConversationsPending { readers, watches } => {
            Some(ShutdownFailure::ConversationsUnreported { readers, watches })
        }
        ShutdownReport::ServersPending {
            readers,
            watches,
            conversations,
        } => Some(ShutdownFailure::ServersUnreported {
            readers,
            watches,
            conversations,
        }),
        ShutdownReport::NativePending {
            readers,
            watches,
            conversations,
        } => Some(ShutdownFailure::NativeUnreported {
            readers,
            watches,
            conversations,
        }),
        // `Confirmed` returned above; named rather than wildcarded so a new
        // report has to say what it means instead of inheriting "said nothing".
        ShutdownReport::Unreported | ShutdownReport::Confirmed => None,
    };
    Err(RunError::Shutdown(unconfirmed))
}

fn runtime_dependencies(config: &Environment) -> RuntimeDependencies {
    match config.uptime_backend {
        UptimeBackend::Monotonic => RuntimeDependencies::default(),
        UptimeBackend::Fixed(ms) => RuntimeDependencies::fixed_uptime(ms),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The startup warm-ups, as the work outside conversations a refused
/// retirement must account for (ADR 221).
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct StartupWarmUps(Arc<Vec<super::local_auth::StartupWarmUp>>);

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl crate::desktop_runtime::application::BackgroundWork for StartupWarmUps {
    fn may_hold_resources(&self) -> bool {
        self.0
            .iter()
            .any(super::local_auth::StartupWarmUp::may_hold_resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, Stage, STAGE};
    use nessa_auth::application::pairing::PairingWorkerFault;
    use std::future::Ready;
    use std::io::Result as IoResult;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot::Sender;
    use tokio::sync::{oneshot, Notify};

    async fn mcp_cleanup_stage(conversation_failure: bool) {
        let report = Arc::new(Mutex::new(ShutdownReport::Unreported));
        let entered = Arc::new(Notify::new());
        let (release, gate) = oneshot::channel();
        let task_report = report.clone();
        let task_entered = entered.clone();
        let task = tokio::spawn(async move {
            let conversations = if conversation_failure {
                Some(std::future::ready(Err(ConversationError::Audit)))
            } else {
                None::<Ready<Result<(), ConversationError>>>
            };
            passive_cleanup(
                &task_report,
                std::future::ready(Ok(())),
                async { Ok(()) },
                async { Ok(()) },
                conversations,
                async {
                    task_entered.notify_one();
                    gate.await.unwrap();
                },
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await;
        });
        let entered = tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .is_ok();
        let pending = match &*report.lock().unwrap() {
            ShutdownReport::ServersPending {
                readers: Ok(()),
                watches: Ok(()),
                conversations,
            } => {
                if conversation_failure {
                    matches!(conversations, Err(ConversationError::Audit))
                } else {
                    conversations.is_ok()
                }
            }
            _ => false,
        };
        let unfinished = !task.is_finished();
        let released = release.send(()).is_ok();
        task.await.unwrap();
        assert!(
            entered,
            "MCP stop must run even without conversations or after conversation failure"
        );
        assert!(
            released,
            "MCP stage must remain held until explicitly released"
        );
        assert!(
            pending,
            "MCP stage publishes and retains the known conversation outcome"
        );
        assert!(unfinished, "MCP drain remains joined");
        let report = report.lock().unwrap();
        if conversation_failure {
            assert!(matches!(
                &*report,
                ShutdownReport::Failed(ShutdownFailure::Conversations(ConversationError::Audit))
            ));
        } else {
            assert!(matches!(&*report, ShutdownReport::Confirmed));
        }
    }

    #[tokio::test]
    async fn mcp_drain_is_joined_without_conversations() {
        mcp_cleanup_stage(false).await;
    }

    #[tokio::test]
    async fn conversation_failure_waits_for_mcp_drain_before_publication() {
        mcp_cleanup_stage(true).await;
    }

    fn assert_unreported_servers(
        report: &Mutex<ShutdownReport>,
        reader_failure: bool,
        conversation_failure: bool,
    ) {
        let Err(RunError::Shutdown(Some(ShutdownFailure::ServersUnreported {
            readers,
            watches: Ok(()),
            conversations,
        }))) = shutdown_result(report)
        else {
            panic!("MCP stage must retain known cleanup results")
        };
        assert_eq!(readers.is_err(), reader_failure);
        if let Err(readers) = readers {
            assert_eq!(
                readers.outcomes().record(),
                Some(&Err(RecordReadError::WorkerPanicked))
            );
            assert_eq!(readers.outcomes().catalogue(), Some(&Ok(())));
        }
        if conversation_failure {
            assert!(matches!(conversations, Err(ConversationError::Audit)));
        } else {
            assert!(conversations.is_ok());
        }
    }

    #[tokio::test]
    async fn mcp_drain_panic_retains_known_cleanup_outcomes() {
        for reader_failure in [false, true] {
            for conversation_failure in [true, false] {
                let report = Arc::new(Mutex::new(ShutdownReport::Unreported));
                let task_report = report.clone();
                let task = tokio::spawn(async move {
                    passive_cleanup(
                        &task_report,
                        std::future::ready(Ok(())),
                        async {
                            if reader_failure {
                                Err(RecordReadError::WorkerPanicked)
                            } else {
                                Ok(())
                            }
                        },
                        async { Ok(()) },
                        Some(async {
                            if conversation_failure {
                                Err(ConversationError::Audit)
                            } else {
                                Ok(())
                            }
                        }),
                        async { panic!("MCP drain fault") },
                        std::future::ready(Ok(())),
                        Duration::from_secs(30),
                    )
                    .await;
                });
                assert!(task.await.unwrap_err().is_panic());
                assert_unreported_servers(&report, reader_failure, conversation_failure);
            }
        }
    }

    #[tokio::test]
    async fn cancelled_mcp_drain_retains_known_cleanup_outcomes() {
        for reader_failure in [false, true] {
            for conversation_failure in [true, false] {
                let report = Mutex::new(ShutdownReport::Unreported);
                {
                    let stop = passive_cleanup(
                        &report,
                        std::future::ready(Ok(())),
                        async {
                            if reader_failure {
                                Err(RecordReadError::WorkerPanicked)
                            } else {
                                Ok(())
                            }
                        },
                        async { Ok(()) },
                        Some(async {
                            if conversation_failure {
                                Err(ConversationError::Audit)
                            } else {
                                Ok(())
                            }
                        }),
                        std::future::pending(),
                        std::future::ready(Ok(())),
                        Duration::from_secs(30),
                    );
                    tokio::pin!(stop);
                    assert!(
                        std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending()))
                            .await
                    );
                }
                assert_unreported_servers(&report, reader_failure, conversation_failure);
            }
        }
    }

    #[tokio::test]
    async fn shutdown_stops_connection_admission_before_a_blocked_reader_drains() {
        struct ObservedListener {
            inner: Option<TcpListener>,
            closed: Option<Sender<()>>,
        }
        impl Listener for ObservedListener {
            type Io = TcpStream;
            type Addr = SocketAddr;
            async fn accept(&mut self) -> (Self::Io, Self::Addr) {
                Listener::accept(self.inner.as_mut().unwrap()).await
            }
            fn local_addr(&self) -> IoResult<Self::Addr> {
                self.inner.as_ref().unwrap().local_addr()
            }
        }
        impl Drop for ObservedListener {
            fn drop(&mut self) {
                drop(self.inner.take());
                let _ = self.closed.take().unwrap().send(());
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (closed, stop) = oneshot::channel();
        let listener = ObservedListener {
            inner: Some(listener),
            closed: Some(closed),
        };
        let router = Router::new().route("/health", axum::routing::get(|| async { "ready" }));
        let report = Arc::new(Mutex::new(ShutdownReport::Confirmed));
        let slot = report.clone();
        let (signal, shutdown) = oneshot::channel();
        let (release, reader) = oneshot::channel();
        let entered = Arc::new(Notify::new());
        let started = entered.clone();
        let conversations = Arc::new(AtomicBool::new(false));
        let cleaned = conversations.clone();
        let task = tokio::spawn(serve_with_cleanup(
            listener,
            router,
            async move {
                shutdown.await.unwrap();
            },
            async move {
                passive_cleanup(
                    &slot,
                    std::future::ready(Ok(())),
                    async {
                        started.notify_one();
                        reader.await.unwrap();
                        Ok(())
                    },
                    async { Ok(()) },
                    Some(async {
                        cleaned.store(true, Ordering::SeqCst);
                        Ok(())
                    }),
                    async {},
                    std::future::ready(Ok(())),
                    Duration::from_secs(30),
                )
                .await;
            },
        ));
        // Prove the same listener admits before the shutdown signal.
        drop(TcpStream::connect(address).await.unwrap());
        signal.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        let admission_stopped = tokio::time::timeout(Duration::from_secs(2), stop)
            .await
            .is_ok();
        let prematurely_finished = task.is_finished();
        let conversations_started = conversations.load(Ordering::SeqCst);
        release.send(()).unwrap();
        let (served, cleanup) = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        served.unwrap();
        cleanup.unwrap();
        assert!(
            admission_stopped,
            "listener remained open while reader cleanup was blocked"
        );
        assert!(!prematurely_finished);
        assert!(!conversations_started);
        assert!(conversations.load(Ordering::SeqCst));
        assert!(shutdown_result(&report).is_ok());
    }

    #[tokio::test]
    async fn cleanup_owner_panic_is_returned_to_process_composition() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let report = Arc::new(Mutex::new(ShutdownReport::Unreported));
        let slot = report.clone();
        let (served, cleanup) = tokio::time::timeout(
            Duration::from_secs(2),
            serve_with_cleanup(listener, Router::new(), async {}, async move {
                passive_cleanup(
                    &slot,
                    std::future::ready(Ok(())),
                    async { Ok(()) },
                    async { Ok(()) },
                    Some(async { Ok(()) }),
                    async {},
                    std::future::ready(Ok(())),
                    Duration::from_secs(30),
                )
                .await;
                panic!("unexpected cleanup fault after publication");
            }),
        )
        .await
        .unwrap();
        served.unwrap();
        assert!(shutdown_result(&report).is_ok());
        assert!(cleanup.unwrap_err().is_panic());
    }

    #[tokio::test]
    async fn shutdown_owner_outcomes_preserve_each_independent_failure() {
        for record_fails in [false, true] {
            for catalogue_fails in [false, true] {
                for conversation_fails in [false, true] {
                    let report = Mutex::new(ShutdownReport::Confirmed);
                    let record = if record_fails {
                        Err(RecordReadError::WorkerPanicked)
                    } else {
                        Ok(())
                    };
                    let catalogue = if catalogue_fails {
                        Err(CatalogueReadError::WorkerPanicked)
                    } else {
                        Ok(())
                    };
                    let cleaned = AtomicBool::new(false);
                    passive_cleanup(
                        &report,
                        std::future::ready(Ok(())),
                        std::future::ready(record),
                        std::future::ready(catalogue.clone()),
                        Some(async {
                            cleaned.store(true, Ordering::SeqCst);
                            if conversation_fails {
                                Err(ConversationError::Audit)
                            } else {
                                Ok(())
                            }
                        }),
                        async {},
                        std::future::ready(Ok(())),
                        Duration::from_secs(30),
                    )
                    .await;
                    assert!(cleaned.load(Ordering::SeqCst));
                    let outcome = shutdown_result(&report);
                    match outcome {
                        Ok(()) => assert!(!record_fails && !catalogue_fails && !conversation_fails),
                        Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
                            ConversationError::Audit,
                        )))) => assert!(!record_fails && !catalogue_fails && conversation_fails),
                        Err(RunError::Shutdown(Some(ShutdownFailure::Readers(readers)))) => {
                            assert!(!conversation_fails);
                            assert_eq!(readers.outcomes().record(), Some(&record));
                            assert_eq!(readers.outcomes().catalogue(), Some(&catalogue));
                            assert!(!readers.outcomes().deadline_exceeded());
                        }
                        Err(RunError::Shutdown(Some(ShutdownFailure::Both {
                            readers,
                            conversations: ConversationError::Audit,
                        }))) => {
                            assert!(conversation_fails);
                            assert_eq!(readers.outcomes().record(), Some(&record));
                            assert_eq!(readers.outcomes().catalogue(), Some(&catalogue));
                        }
                        other => panic!("cleanup causes changed: {other:?}"),
                    }
                }
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn gated_second_reader_retains_first_fault_deadline_and_eventual_both_causes() {
        let (release, waiting) = oneshot::channel();
        let report = Arc::new(Mutex::new(ShutdownReport::Confirmed));
        let output = report.clone();
        let cleaned = Arc::new(AtomicBool::new(false));
        let cleanup = cleaned.clone();
        let task = tokio::spawn(async move {
            passive_cleanup(
                &output,
                std::future::ready(Ok(())),
                std::future::ready(Err(RecordReadError::WorkerPanicked)),
                async {
                    waiting.await.unwrap();
                    Err(CatalogueReadError::WorkerPanicked)
                },
                Some(async {
                    cleanup.store(true, Ordering::SeqCst);
                    Err(ConversationError::Audit)
                }),
                async {},
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        assert!(!cleaned.load(Ordering::SeqCst));
        assert!(!task.is_finished());
        release.send(()).unwrap();
        task.await.unwrap();
        let Err(RunError::Shutdown(Some(ShutdownFailure::Both {
            readers,
            conversations: ConversationError::Audit,
        }))) = shutdown_result(&report)
        else {
            panic!("all cleanup causes must survive")
        };
        assert_eq!(
            readers.outcomes().record(),
            Some(&Err(RecordReadError::WorkerPanicked))
        );
        assert_eq!(
            readers.outcomes().catalogue(),
            Some(&Err(CatalogueReadError::WorkerPanicked))
        );
        assert!(readers.outcomes().deadline_exceeded());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_second_reader_preserves_known_fault_or_success_before_and_after_deadline() {
        for result in [Ok(()), Err(RecordReadError::WorkerPanicked)] {
            for after_deadline in [false, true] {
                let report = Mutex::new(ShutdownReport::Confirmed);
                let cleaned = AtomicBool::new(false);
                {
                    let stop = passive_cleanup(
                        &report,
                        std::future::ready(Ok(())),
                        std::future::ready(result),
                        std::future::pending(),
                        Some(async {
                            cleaned.store(true, Ordering::SeqCst);
                            Ok(())
                        }),
                        async {},
                        std::future::ready(Ok(())),
                        Duration::from_secs(30),
                    );
                    tokio::pin!(stop);
                    assert!(
                        std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending()))
                            .await
                    );
                    if after_deadline {
                        tokio::time::advance(Duration::from_secs(31)).await;
                        assert!(
                            std::future::poll_fn(|cx| Poll::Ready(
                                stop.as_mut().poll(cx).is_pending()
                            ))
                            .await
                        );
                    }
                }
                let Err(RunError::Shutdown(Some(ShutdownFailure::DrainsUnreported {
                    outcomes,
                    ..
                }))) = shutdown_result(&report)
                else {
                    panic!("pending reader evidence must survive")
                };
                assert_eq!(outcomes.record(), Some(&result));
                assert_eq!(outcomes.catalogue(), None);
                assert_eq!(outcomes.deadline_exceeded(), after_deadline);
                assert!(!cleaned.load(Ordering::SeqCst));
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_before_either_reader_outcome_retains_unknown_drain_and_deadline() {
        for after_deadline in [false, true] {
            let report = Mutex::new(ShutdownReport::Confirmed);
            {
                let stop = passive_cleanup(
                    &report,
                    std::future::ready(Ok(())),
                    std::future::pending(),
                    std::future::pending(),
                    Some(std::future::ready(Ok(()))),
                    async {},
                    std::future::ready(Ok(())),
                    Duration::from_secs(30),
                );
                tokio::pin!(stop);
                assert!(
                    std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending()))
                        .await
                );
                if after_deadline {
                    tokio::time::advance(Duration::from_secs(31)).await;
                    assert!(
                        std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending()))
                            .await
                    );
                }
            }
            let Err(RunError::Shutdown(Some(ShutdownFailure::DrainsUnreported {
                outcomes, ..
            }))) = shutdown_result(&report)
            else {
                panic!("unknown is not successful")
            };
            assert_eq!(outcomes.record(), None);
            assert_eq!(outcomes.catalogue(), None);
            assert_eq!(outcomes.deadline_exceeded(), after_deadline);
        }
    }

    #[tokio::test]
    async fn cancelled_conversation_cleanup_retains_complete_reader_evidence() {
        for result in [Ok(()), Err(RecordReadError::WorkerPanicked)] {
            let report = Mutex::new(ShutdownReport::Confirmed);
            {
                let stop = passive_cleanup(
                    &report,
                    std::future::ready(Ok(())),
                    std::future::ready(result),
                    std::future::ready(Ok(())),
                    Some(std::future::pending()),
                    async {},
                    std::future::ready(Ok(())),
                    Duration::from_secs(30),
                );
                tokio::pin!(stop);
                assert!(
                    std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending()))
                        .await
                );
            }
            let Err(RunError::Shutdown(Some(ShutdownFailure::ConversationsUnreported {
                readers,
                watches: Ok(()),
            }))) = shutdown_result(&report)
            else {
                panic!("conversation completion remains unknown")
            };
            match (result, readers) {
                (Ok(()), Ok(())) => {}
                (Err(error), Err(readers)) => {
                    assert_eq!(readers.outcomes().record(), Some(&Err(error)));
                    assert_eq!(readers.outcomes().catalogue(), Some(&Ok(())));
                }
                other => panic!("known evidence changed: {other:?}"),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reader_deadline_retains_eventual_successful_drain() {
        let report = Mutex::new(ShutdownReport::Confirmed);
        passive_cleanup(
            &report,
            std::future::ready(Ok(())),
            async {
                tokio::time::sleep(Duration::from_secs(31)).await;
                Ok(())
            },
            std::future::ready(Ok(())),
            Some(std::future::ready(Ok(()))),
            async {},
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
        let Err(RunError::Shutdown(Some(ShutdownFailure::Readers(readers)))) =
            shutdown_result(&report)
        else {
            panic!("deadline evidence must survive successful drains")
        };
        assert!(readers.outcomes().deadline_exceeded());
        assert_eq!(readers.outcomes().record(), Some(&Ok(())));
        assert_eq!(readers.outcomes().catalogue(), Some(&Ok(())));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_conversation_after_deadline_retains_both_reader_results() {
        let report = Mutex::new(ShutdownReport::Confirmed);
        let (release, waiting) = oneshot::channel();
        {
            let stop = passive_cleanup(
                &report,
                std::future::ready(Ok(())),
                std::future::ready(Ok(())),
                async {
                    waiting.await.unwrap();
                    Err(CatalogueReadError::OperationAndWorkerPanicked(Box::new(
                        CatalogueReadError::IdentityChanged,
                    )))
                },
                Some(std::future::pending()),
                async {},
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            );
            tokio::pin!(stop);
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await
            );
            tokio::time::advance(Duration::from_secs(31)).await;
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await
            );
            release.send(()).unwrap();
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await
            );
        }
        let Err(RunError::Shutdown(Some(ShutdownFailure::ConversationsUnreported {
            watches: Ok(()),
            readers: Err(readers),
        }))) = shutdown_result(&report)
        else {
            panic!("retain typed catalogue failure and unknown conversation cleanup")
        };
        assert!(readers.outcomes().deadline_exceeded());
        assert_eq!(readers.outcomes().record(), Some(&Ok(())));
        assert_eq!(
            readers.outcomes().catalogue(),
            Some(&Err(CatalogueReadError::OperationAndWorkerPanicked(
                Box::new(CatalogueReadError::IdentityChanged)
            )))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ready_readers_at_zero_deadline_are_not_labeled_timeout() {
        let report = Mutex::new(ShutdownReport::Confirmed);
        passive_cleanup(
            &report,
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            Some(std::future::ready(Ok(()))),
            async {},
            std::future::ready(Ok(())),
            Duration::ZERO,
        )
        .await;
        assert!(shutdown_result(&report).is_ok());
    }

    #[test]
    fn fixed_clock_flows_from_config_to_runtime() {
        for stage in ["dev", "ci"] {
            let config = Environment::load(
                &MockEnv::new()
                    .set("NESSA_STAGE", stage)
                    .set("NESSA_UPTIME_BACKEND", "fixed")
                    .set("NESSA_UPTIME_FIXED_MS", "123"),
            )
            .unwrap();
            assert_eq!(runtime_dependencies(&config).clock.elapsed_ms(), 123);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn desktop_runtime_signals_are_caught_by_the_server_process() {
        let mut stop_agents = signal(SignalKind::user_defined1()).unwrap();
        let mut retire = signal(SignalKind::user_defined2()).unwrap();

        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
        tokio::time::timeout(std::time::Duration::from_secs(1), stop_agents.recv())
            .await
            .expect("USR1 handler did not receive the process signal")
            .expect("USR1 signal stream closed");

        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR2) }, 0);
        tokio::time::timeout(std::time::Duration::from_secs(1), retire.recv())
            .await
            .expect("USR2 handler did not receive the process signal")
            .expect("USR2 signal stream closed");
    }
    #[test]
    fn a_completed_serve_still_fails_when_shutdown_did_not_confirm_cleanup() {
        let confirmed = Mutex::new(ShutdownReport::Confirmed);
        assert!(shutdown_result(&confirmed).is_ok());

        let undrained = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::RetirementAdmission {
                cleanup_error: Some(Box::new(ConversationError::Audit)),
            },
        )));
        // The typed failure survives the boundary rather than becoming a log line.
        let Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
            ConversationError::RetirementAdmission { cleanup_error },
        )))) = shutdown_result(&undrained)
        else {
            panic!("undrained admission must fail the process result")
        };
        assert!(matches!(
            cleanup_error.as_deref(),
            Some(ConversationError::Audit)
        ));
        // Taken once, and what is left behind is not confirmation.
        assert!(matches!(
            shutdown_result(&undrained),
            Err(RunError::Shutdown(None))
        ));
        // A run that was confirmed stays confirmed, however often it is read.
        let confirmed_twice = Mutex::new(ShutdownReport::Confirmed);
        assert!(shutdown_result(&confirmed_twice).is_ok());
        assert!(shutdown_result(&confirmed_twice).is_ok());

        let audit = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::Audit,
        )));
        assert!(matches!(
            shutdown_result(&audit),
            Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
                ConversationError::Audit
            ))))
        ));
    }

    #[test]
    fn the_process_result_carries_both_serving_and_what_shutdown_reported() {
        // Serving finishing is not confirmation that conversations stopped.
        let unconfirmed = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::Audit,
        )));
        assert!(matches!(
            serve_outcome(Ok(()), None, &unconfirmed),
            Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
                ConversationError::Audit
            ))))
        ));

        let confirmed = Mutex::new(ShutdownReport::Confirmed);
        assert!(serve_outcome(Ok(()), None, &confirmed).is_ok());

        // A serve failure is the fault that stopped the process, so it wins —
        // but the report is read first, so it cannot be skipped past.
        let both = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::Audit,
        )));
        let served = Err(std::io::Error::other("listener died"));
        assert!(matches!(
            serve_outcome(served, None, &both),
            Err(RunError::Serve(_))
        ));
        assert!(matches!(*both.lock().unwrap(), ShutdownReport::Unreported));
    }

    #[tokio::test]
    async fn a_shutdown_that_finishes_records_which_way_it_went() {
        let confirmed = Mutex::new(ShutdownReport::Unreported);
        passive_cleanup(
            &confirmed,
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            Some(std::future::ready(Ok(()))),
            async {},
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
        assert!(shutdown_result(&confirmed).is_ok());

        let failed = Mutex::new(ShutdownReport::Confirmed);
        passive_cleanup(
            &failed,
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            Some(std::future::ready(Err(ConversationError::Audit))),
            async {},
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(
            shutdown_result(&failed),
            Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
                ConversationError::Audit
            ))))
        ));
    }

    #[test]
    fn a_shutdown_that_never_reported_is_not_treated_as_confirmed() {
        // Armed before the await and never cleared: the cleanup owner did not finish.
        let unreported = Mutex::new(ShutdownReport::Unreported);
        let error = shutdown_result(&unreported).unwrap_err();
        assert!(matches!(error, RunError::Shutdown(None)));
        // Said plainly, rather than borrowing another failure's meaning.
        assert!(error.to_string().contains("never reported"));

        // A poisoned slot is read through, so a failure already recorded is
        // still the answer rather than being downgraded to "never reported".
        let poisoned = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::Audit,
        )));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poisoned.lock().unwrap();
            panic!("panicked while the report was held")
        }));
        assert!(matches!(
            shutdown_result(&poisoned),
            Err(RunError::Shutdown(Some(ShutdownFailure::Conversations(
                ConversationError::Audit
            ))))
        ));
    }

    /// Row S12: native pairing's drain is joined into the same report, and its
    /// failure is kept beside whatever the earlier stages said.
    #[tokio::test]
    async fn native_shutdown_failure_is_retained_beside_other_cleanup() {
        let fault = NativeShutdownFailure::ListenerFault(PairingWorkerFault::Panic);
        for conversation_fails in [false, true] {
            let report = Mutex::new(ShutdownReport::Unreported);
            let servers_stopped = AtomicBool::new(false);
            passive_cleanup(
                &report,
                std::future::ready(Ok(())),
                std::future::ready(Ok(())),
                std::future::ready(Ok(())),
                Some(std::future::ready(if conversation_fails {
                    Err(ConversationError::Audit)
                } else {
                    Ok(())
                })),
                async { servers_stopped.store(true, Ordering::SeqCst) },
                async {
                    // MCP stop has returned before native's join is observed.
                    assert!(servers_stopped.load(Ordering::SeqCst));
                    Err(fault)
                },
                Duration::from_secs(30),
            )
            .await;
            let Err(RunError::Shutdown(Some(ShutdownFailure::Native {
                readers: Ok(()),
                watches: Ok(()),
                conversations,
                native,
            }))) = shutdown_result(&report)
            else {
                panic!("a native drain failure must survive into the result");
            };
            assert_eq!(native, fault);
            assert_eq!(conversations.is_err(), conversation_fails);
        }
        // A confirmed native join leaves an otherwise clean report confirmed.
        let report = Mutex::new(ShutdownReport::Unreported);
        passive_cleanup(
            &report,
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            None::<Ready<Result<(), ConversationError>>>,
            async {},
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
        assert!(shutdown_result(&report).is_ok());
    }

    /// Row S12: a report taken while native's drain is still pending says so;
    /// it is never Confirmed.
    #[tokio::test]
    async fn unreported_native_drain_is_not_confirmed() {
        let report = Mutex::new(ShutdownReport::Unreported);
        {
            let stop = passive_cleanup(
                &report,
                std::future::ready(Ok(())),
                std::future::ready(Ok(())),
                std::future::ready(Ok(())),
                Some(std::future::ready(Err(ConversationError::Audit))),
                async {},
                std::future::pending(),
                Duration::from_secs(30),
            );
            tokio::pin!(stop);
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await
            );
        }
        let Err(RunError::Shutdown(Some(ShutdownFailure::NativeUnreported {
            readers: Ok(()),
            watches: Ok(()),
            conversations: Err(ConversationError::Audit),
        }))) = shutdown_result(&report)
        else {
            panic!("a pending native drain must not read as confirmed");
        };
    }

    /// Rows S12 and H2 together: a watch task fault and a native drain failure
    /// in the same shutdown are both kept in the one report.
    #[tokio::test]
    async fn watch_and_native_shutdown_failures_are_both_retained() {
        let fault = NativeShutdownFailure::ListenerFault(PairingWorkerFault::Panic);
        let report = Mutex::new(ShutdownReport::Unreported);
        passive_cleanup(
            &report,
            std::future::ready(Err(WatchTaskFault::Panic)),
            std::future::ready(Ok(())),
            std::future::ready(Ok(())),
            None::<Ready<Result<(), ConversationError>>>,
            async {},
            std::future::ready(Err(fault)),
            Duration::from_secs(30),
        )
        .await;
        let Err(RunError::Shutdown(Some(ShutdownFailure::Native {
            readers: Ok(()),
            watches: Err(watches),
            conversations: Ok(()),
            native,
        }))) = shutdown_result(&report)
        else {
            panic!("both failures must survive into the result");
        };
        assert_eq!(native, fault);
        assert_eq!(watches.fault(), Some(WatchTaskFault::Panic));
    }

    /// Row S14: a native listener failure stops the gateway like a process
    /// signal and is the process result, read after the shutdown report.
    #[tokio::test]
    async fn native_listener_failure_stops_the_gateway_and_is_the_process_result() {
        let (failure, failed) = tokio::sync::watch::channel(None);
        let stop = tokio::spawn(stop_signal(std::future::pending(), Some(failed.clone())));
        tokio::task::yield_now().await;
        assert!(!stop.is_finished(), "nothing has failed yet");
        failure.send_replace(Some(std::io::ErrorKind::InvalidInput));
        tokio::time::timeout(Duration::from_secs(2), stop)
            .await
            .expect("the failure resolves the stop signal")
            .unwrap();
        let report = Mutex::new(ShutdownReport::Failed(ShutdownFailure::Conversations(
            ConversationError::Audit,
        )));
        assert!(matches!(
            serve_outcome(Ok(()), *failed.borrow(), &report),
            Err(RunError::Native(NativeFailure::Listener(
                std::io::ErrorKind::InvalidInput
            )))
        ));
        assert!(
            matches!(*report.lock().unwrap(), ShutdownReport::Unreported),
            "the report was read before the listener failure was returned"
        );
        // With native pairing off there is no channel; only the process
        // signal stops the gateway.
        let (signal, signalled) = oneshot::channel::<()>();
        let stop = tokio::spawn(stop_signal(
            async {
                let _ = signalled.await;
            },
            None,
        ));
        tokio::task::yield_now().await;
        assert!(!stop.is_finished(), "nothing native can stop it");
        signal.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), stop)
            .await
            .unwrap()
            .unwrap();
    }

    /// Row S15: a native listener task that ends without publishing a failure,
    /// as on a panic, drops its sender; that also stops the gateway.
    #[tokio::test]
    async fn a_vanished_native_listener_stops_the_gateway() {
        let (failure, failed) = tokio::sync::watch::channel(None);
        let listener = tokio::spawn(async move {
            let _failure = failure;
            panic!("native listener fault");
        });
        assert!(listener.await.unwrap_err().is_panic());
        tokio::time::timeout(
            Duration::from_secs(2),
            stop_signal(std::future::pending(), Some(failed.clone())),
        )
        .await
        .expect("a vanished listener resolves the stop signal");
        // It is no listener failure: the shutdown report carries the fault.
        let report = Mutex::new(ShutdownReport::Confirmed);
        assert!(serve_outcome(Ok(()), *failed.borrow(), &report).is_ok());
    }

    #[test]
    fn environment_loads_for_ci_stage() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("ci config");
        assert_eq!(config.stage, Stage::Ci);
    }
}

#[cfg(test)]
#[path = "../../tests/composition/watch_shutdown.rs"]
mod watch_shutdown_tests;
