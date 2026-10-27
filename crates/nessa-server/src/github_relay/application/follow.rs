use super::{catch_up, GitHubCredentials, RelayLiveSource, RelayReporter, RelayStore, RelayWait};
use crate::github_relay::domain::{Binding, Failure};
use futures_util::FutureExt;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinSet;

/// Retires removed generations and joins failed tasks. Dropping this future
/// drops its JoinSet, which aborts the remaining followers.
pub async fn serve(
    source: Arc<dyn RelayLiveSource>,
    store: Arc<dyn RelayStore>,
    credentials: Arc<dyn GitHubCredentials>,
    wait: Arc<dyn RelayWait>,
    reporter: Arc<dyn RelayReporter>,
) {
    let mut tasks = JoinSet::new();
    let mut started = HashMap::new();
    loop {
        match store.bindings().await {
            Ok(bindings) => {
                let active: HashSet<_> = bindings
                    .iter()
                    .map(|binding| {
                        (
                            binding.user_id().to_owned(),
                            binding.repository_id().to_owned(),
                            binding.generation(),
                        )
                    })
                    .collect();
                started.retain(|key, handle: &mut tokio::task::AbortHandle| {
                    if !active.contains(key) || handle.is_finished() {
                        handle.abort();
                        false
                    } else {
                        true
                    }
                });
                for binding in bindings {
                    let key = (
                        binding.user_id().to_owned(),
                        binding.repository_id().to_owned(),
                        binding.generation(),
                    );
                    started.entry(key).or_insert_with(|| {
                        let (source, store, credentials, wait, reporter) = (
                            source.clone(),
                            store.clone(),
                            credentials.clone(),
                            wait.clone(),
                            reporter.clone(),
                        );
                        tasks.spawn(async move {
                            follow(
                                source.as_ref(),
                                store.as_ref(),
                                credentials.as_ref(),
                                &binding,
                                wait.as_ref(),
                                reporter.as_ref(),
                            )
                            .await
                        })
                    });
                }
            }
            Err(Failure::VaultLocked) => {
                for (_, handle) in started.drain() {
                    handle.abort();
                }
            }
            Err(error) => reporter.storage_failed(&error),
        }
        // Observe task panics; normal followers run until this owner is dropped.
        while let Some(Some(result)) = tasks.join_next().now_or_never() {
            if result.is_err_and(|error| !error.is_cancelled()) {
                reporter.task_failed();
            }
        }
        wait.wait(Duration::from_secs(5)).await;
    }
}
/// Subscribe before the second catch-up read; renew to recover a lost wake-up.
pub async fn follow(
    source: &dyn RelayLiveSource,
    store: &dyn RelayStore,
    credentials: &dyn GitHubCredentials,
    binding: &Binding,
    wait: &dyn RelayWait,
    reporter: &dyn RelayReporter,
) {
    let mut retry = 1u64;
    loop {
        let result=async {
            let token=credentials.token().await?;
            // Obtain a ticket, subscribe, then re-read: the second read closes
            // the catch-up/watch race even if the first notification was lost.
            let page=catch_up(source,store,binding,&token).await?;
            let mut watch=source.watch(&page.ticket).await?;
            catch_up(source,store,binding,&token).await?;
            let renew=wait.wait(Duration::from_secs(crate::github_relay::domain::contract::RENEW_AFTER_SECONDS));tokio::pin!(renew);
            loop {
                tokio::select! {
                    _=&mut renew=>return Ok::<(),Failure>(()),
                    changed=watch.changed()=>{ changed?;catch_up(source,store,binding,&token).await?; }
                }
            }
        }.await;
        match result {
            Ok(()) => {
                retry = 1;
            }
            Err(error) => {
                reporter.disconnected(binding, &error);
                let delay = if matches!(
                    error,
                    Failure::Forbidden
                        | Failure::RateLimited
                        | Failure::IdentityChanged
                        | Failure::Gap(_)
                        | Failure::CredentialsUnavailable
                ) {
                    300
                } else {
                    retry
                };
                wait.wait(Duration::from_secs(delay)).await;
                retry = (retry * 2).min(60);
            }
        }
    }
}
