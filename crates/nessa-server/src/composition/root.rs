use crate::cli::entrypoint::{Command, LocalProvisioning, HELP};
use crate::conversation::application::ConversationError;
#[cfg(target_os = "macos")]
use crate::desktop_runtime::{
    application::{restore_retirement, retire},
    infrastructure::RetirementFiles,
};
use crate::env::Environment;
use crate::server::entrypoint::http;
use crate::{
    app::dependencies::RuntimeDependencies,
    core::{Launch, RunError},
    env::UptimeBackend,
};
use axum::Extension;
use std::future::Future;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
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
            warm_ups,
        } = super::local_auth::product_state(&config, dependencies.clock.clone(), bundle)?;
        let conversations = product.conversations.clone();
        #[cfg(target_os = "macos")]
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
        #[cfg(target_os = "macos")]
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
        let mut router = http::router(product);
        if let Some(identity) = &desktop_identity {
            router = router.layer(Extension(identity.clone()));
        }

        let listen_addr = config.listen_addr();
        let listener = tokio::net::TcpListener::bind(&listen_addr)
            .await
            .map_err(|source| RunError::Bind {
                addr: listen_addr.clone(),
                source,
            })?;

        tracing::info!(
            listen_addr = %listen_addr,
            stage = config.stage.as_str(),
            "nessa server listening",
        );

        // Only now: the runtime's first launch is slow because the operating
        // system scans it, and that wait belongs here, with the window already
        // up, rather than inside the user's first message.
        for warm_up in &warm_ups {
            warm_up.start();
        }

        #[cfg(target_os = "macos")]
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
        #[cfg(target_os = "macos")]
        if let Some(identity) = desktop_identity {
            let files = retirement_files.expect("desktop files initialized before admission");
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
                    let result =
                        retire(request, identity.clone(), service.as_deref(), &files).await;
                    if let Err(error) = files.result(&result) {
                        tracing::error!(%error, "could not acknowledge desktop retirement");
                    }
                }
            });
        }
        // Axum's graceful-shutdown callback has nowhere to return a failure, and
        // the process exiting zero would report cleanup this never confirmed.
        // The outcome is carried back out of the callback instead of logged away.
        //
        // The slot is armed before shutdown is awaited and cleared only by a
        // confirmed stop, so a callback that panics partway leaves it armed:
        // never hearing back is its own fact, and it is not confirmation.
        let report: Arc<Mutex<ShutdownReport>> = Arc::new(Mutex::new(ShutdownReport::Confirmed));
        let slot = report.clone();
        let served = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown_signal().await;
                let Some(service) = conversations else { return };
                record_conversation_shutdown(&slot, service.shutdown()).await;
            })
            .await;
        serve_outcome(served, &report)
    }
}

/// This process's result, from serving and from what shutdown reported.
///
/// The report is read before `served` is propagated, so a serve error cannot
/// discard it. Axum 0.8's graceful-shutdown future always ends in `Ok(())`, so
/// `served` is vestigial today; it is taken and propagated rather than ignored
/// in case a later axum gives the serve loop an error path again.
fn serve_outcome(
    served: std::io::Result<()>,
    report: &Mutex<ShutdownReport>,
) -> Result<(), RunError> {
    let unconfirmed = shutdown_result(report);
    served?;
    unconfirmed
}

/// What graceful shutdown managed to say about itself.
#[derive(Debug)]
enum ShutdownReport {
    /// Nothing is outstanding: conversations confirmed their cleanup and audit
    /// delivery, or there were none to stop and no shutdown ever began. This is
    /// also the slot's starting value, which is what makes those the same fact.
    Confirmed,
    /// Shutdown began and said nothing further — the callback did not finish.
    Unreported,
    /// Shutdown finished and reported this failure.
    Failed(ConversationError),
}

/// Stop conversations, recording whatever that manages to say about itself.
///
/// The slot is armed before `stop` is awaited and written again only by an
/// outcome, so a shutdown that is cancelled or panics partway leaves
/// `Unreported` behind rather than the `Confirmed` the slot started life with.
/// Axum runs this callback in a task of its own, so that is a reachable ending
/// and not a theoretical one.
async fn record_conversation_shutdown(
    slot: &Mutex<ShutdownReport>,
    stop: impl Future<Output = Result<(), ConversationError>>,
) {
    record_shutdown(slot, ShutdownReport::Unreported);
    match stop.await {
        Ok(()) => record_shutdown(slot, ShutdownReport::Confirmed),
        Err(error) => {
            tracing::error!(%error, "conversation shutdown did not confirm all cleanup");
            record_shutdown(slot, ShutdownReport::Failed(error));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, Stage, STAGE};

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
    #[test]
    fn a_completed_serve_still_fails_when_shutdown_did_not_confirm_cleanup() {
        let confirmed = Mutex::new(ShutdownReport::Confirmed);
        assert!(shutdown_result(&confirmed).is_ok());

        let undrained = Mutex::new(ShutdownReport::Failed(
            ConversationError::RetirementAdmission {
                cleanup_error: Some(Box::new(ConversationError::Audit)),
            },
        ));
        // The typed failure survives the boundary rather than becoming a log line.
        let Err(RunError::Shutdown(Some(ConversationError::RetirementAdmission { cleanup_error }))) =
            shutdown_result(&undrained)
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

        let audit = Mutex::new(ShutdownReport::Failed(ConversationError::Audit));
        assert!(matches!(
            shutdown_result(&audit),
            Err(RunError::Shutdown(Some(ConversationError::Audit)))
        ));
    }

    #[test]
    fn the_process_result_carries_both_serving_and_what_shutdown_reported() {
        // Serving finishing is not confirmation that conversations stopped.
        let unconfirmed = Mutex::new(ShutdownReport::Failed(ConversationError::Audit));
        assert!(matches!(
            serve_outcome(Ok(()), &unconfirmed),
            Err(RunError::Shutdown(Some(ConversationError::Audit)))
        ));

        let confirmed = Mutex::new(ShutdownReport::Confirmed);
        assert!(serve_outcome(Ok(()), &confirmed).is_ok());

        // A serve failure is the fault that stopped the process, so it wins —
        // but the report is read first, so it cannot be skipped past.
        let both = Mutex::new(ShutdownReport::Failed(ConversationError::Audit));
        let served = Err(std::io::Error::other("listener died"));
        assert!(matches!(
            serve_outcome(served, &both),
            Err(RunError::Serve(_))
        ));
        assert!(matches!(*both.lock().unwrap(), ShutdownReport::Unreported));
    }

    #[tokio::test]
    async fn a_shutdown_that_is_dropped_partway_leaves_no_confirmation() {
        // What the arming is for: the slot starts as `Confirmed`, so a stop that
        // never finishes must have moved it off that before it began waiting.
        let slot = Mutex::new(ShutdownReport::Confirmed);
        {
            let stop = record_conversation_shutdown(
                &slot,
                std::future::pending::<Result<(), ConversationError>>(),
            );
            tokio::pin!(stop);
            tokio::select! {
                biased;
                _ = &mut stop => unreachable!("a pending stop cannot finish"),
                _ = std::future::ready(()) => {}
            }
            // `stop` is dropped here, mid-await, as a cancelled task would be.
        }
        assert!(matches!(
            shutdown_result(&slot),
            Err(RunError::Shutdown(None))
        ));
    }

    #[tokio::test]
    async fn a_shutdown_that_finishes_records_which_way_it_went() {
        let confirmed = Mutex::new(ShutdownReport::Unreported);
        record_conversation_shutdown(&confirmed, std::future::ready(Ok(()))).await;
        assert!(shutdown_result(&confirmed).is_ok());

        let failed = Mutex::new(ShutdownReport::Confirmed);
        record_conversation_shutdown(&failed, std::future::ready(Err(ConversationError::Audit)))
            .await;
        assert!(matches!(
            shutdown_result(&failed),
            Err(RunError::Shutdown(Some(ConversationError::Audit)))
        ));
    }

    #[test]
    fn a_shutdown_that_never_reported_is_not_treated_as_confirmed() {
        // Armed before the await and never cleared: the callback did not finish.
        let unreported = Mutex::new(ShutdownReport::Unreported);
        let error = shutdown_result(&unreported).unwrap_err();
        assert!(matches!(error, RunError::Shutdown(None)));
        // Said plainly, rather than borrowing another failure's meaning.
        assert!(error.to_string().contains("never reported"));

        // A poisoned slot is read through, so a failure already recorded is
        // still the answer rather than being downgraded to "never reported".
        let poisoned = Mutex::new(ShutdownReport::Failed(ConversationError::Audit));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poisoned.lock().unwrap();
            panic!("panicked while the report was held")
        }));
        assert!(matches!(
            shutdown_result(&poisoned),
            Err(RunError::Shutdown(Some(ConversationError::Audit)))
        ));
    }

    #[test]
    fn environment_loads_for_ci_stage() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("ci config");
        assert_eq!(config.stage, Stage::Ci);
    }
}
