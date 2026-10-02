//! Composition of one finite authenticated socket run and its private cache.
use super::profile::{Profile, ProfileError};
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::conversation::domain::ConversationId;
use crate::product::generated::{MAX_AUTH_CREDENTIAL_CHARACTERS, PASSIVE_MIN_REQUEST_TIMEOUT_MS};
use crate::read_only_sync::application::driver::{run_catalogue, run_records};
use crate::read_only_sync::application::{CachePolicy, Cancellation, GatewayError, GatewayPolicy};
use crate::read_only_sync::entrypoint::{online, Command, CommandError};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use crate::read_only_sync::infrastructure::gateway::{GatewayConnection, LocalConnector, Session};
use serde_json::json;
#[cfg(test)]
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    // One-shot scheduling witness for actual composition saved-output races.
    static BEFORE_SAVED_REFRESH: RefCell<Option<Box<dyn FnOnce() + Send>>> = const { RefCell::new(None) };
}

struct CommandCancellation;
impl Cancellation for CommandCancellation {
    fn cancelled(&self) -> bool {
        false
    }
}

pub(super) fn execute(
    command: &Command,
    policy: CachePolicy,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let (cache_path, profile_path) = match command {
        Command::Records { cache, profile, .. } | Command::Catalogue { cache, profile, .. } => {
            (cache, profile)
        }
        Command::Local(_) => return Err(CommandError::Arguments),
    };
    let connection = connect(profile_path, output)?;
    let (profile, gateway) = connection;
    match command {
        Command::Records {
            conversation,
            pages,
            ..
        } => records(
            cache_path,
            policy,
            profile,
            gateway,
            conversation.clone(),
            *pages,
            output,
        ),
        Command::Catalogue { pages, .. } => {
            catalogue(cache_path, policy, profile, gateway, *pages, output)
        }
        Command::Local(_) => Err(CommandError::Arguments),
    }
}
fn profile_code(error: ProfileError) -> &'static str {
    match error {
        ProfileError::Unavailable => "unavailable",
        ProfileError::TooLarge => "tooLarge",
        ProfileError::Invalid => "invalid",
        ProfileError::EndpointUnavailable => "endpointUnavailable",
        ProfileError::CredentialUnavailable => "credentialUnavailable",
        ProfileError::CredentialTooLarge => "credentialTooLarge",
        ProfileError::CredentialEncoding => "credentialEncoding",
    }
}
// B1: one whole-callback budget, not a renewed allowance for each RPC.
// Finite passes may still exhaust this budget while retaining confirmed pages.
fn default_gateway_policy() -> Result<GatewayPolicy, GatewayError> {
    GatewayPolicy::new(5000, PASSIVE_MIN_REQUEST_TIMEOUT_MS, 8192, 16, 16)
}
fn connect(
    path: &Path,
    output: &mut dyn Write,
) -> Result<(Profile, GatewayConnection), CommandError> {
    let inputs = (|| {
        let profile = Profile::load(path)?;
        let endpoint = profile.endpoint()?;
        let credential = profile.credential(MAX_AUTH_CREDENTIAL_CHARACTERS)?;
        Ok::<_, ProfileError>((profile, endpoint, credential))
    })();
    let (profile, endpoint, credential) = match inputs {
        Ok(value) => value,
        Err(error) => {
            online::write(
                json!({"successful":false,"connectionCheck":"notPerformed","configurationFailure":profile_code(error)}),
                output,
            )?;
            return Err(CommandError::OnlineRefused);
        }
    };
    let policy = default_gateway_policy().map_err(CommandError::Gateway)?;
    match Session::connect(
        &endpoint,
        &credential,
        "read-only-example",
        &LocalConnector,
        RuntimeDependencies::default().clock,
        Arc::new(CommandCancellation),
        policy,
    ) {
        Ok(session) => Ok((profile, GatewayConnection::new(session))),
        Err(error) => {
            online::write(
                json!({"successful":false,"connectionCheck":"attempted","transportFailure":online::gateway_failure(error)}),
                output,
            )?;
            Err(CommandError::OnlineRefused)
        }
    }
}
fn records(
    path: &Path,
    policy: CachePolicy,
    profile: Profile,
    connection: GatewayConnection,
    conversation: ConversationId,
    pages: usize,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let mut source = connection.records(profile.receiver, profile.access_epoch, conversation);
    let discovery = connection
        .run(|| source.discover())
        .map_err(CommandError::Gateway)?;
    let scope = match discovery.result {
        Some(Ok((scope, _))) => scope,
        _ => {
            online::write(
                json!({"successful":false,"connectionCheck":"performed","connectionOperation":discovery.outcome.operation.to_string(),"transportFailure":discovery.outcome.failure.map(online::gateway_failure),"discoveryFailure":true}),
                output,
            )?;
            return Err(CommandError::OnlineRefused);
        }
    };
    let mut cache = open_cache(path, policy, output)?;
    let mut authorizer = source.authorizer();
    let attempt = connection
        .run(|| {
            run_records(
                &scope,
                &mut authorizer,
                &mut source,
                &mut cache,
                policy.suffix_page(),
                pages,
                &SystemClock,
            )
        })
        .map_err(CommandError::Gateway)?;
    let refusal = cache.take_refusal();
    let saved = cache.retained_transcript_state(scope.receiver(), scope.origin(), scope.stream());
    online::write_records(&attempt, saved, refusal, output)
}
fn catalogue(
    path: &Path,
    policy: CachePolicy,
    profile: Profile,
    connection: GatewayConnection,
    pages: usize,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let mut source = connection.catalogue(profile.receiver, profile.access_epoch);
    let discovery = connection
        .run(|| source.discover())
        .map_err(CommandError::Gateway)?;
    let scope = match discovery.result {
        Some(Ok((scope, _))) => scope,
        _ => {
            online::write(
                json!({"successful":false,"connectionCheck":"performed","connectionOperation":discovery.outcome.operation.to_string(),"transportFailure":discovery.outcome.failure.map(online::gateway_failure),"discoveryFailure":true}),
                output,
            )?;
            return Err(CommandError::OnlineRefused);
        }
    };
    let mut cache = open_cache(path, policy, output)?;
    let mut authorizer = source.authorizer();
    let attempt = connection
        .run(|| run_catalogue(&scope, &mut authorizer, &mut source, &mut cache, pages))
        .map_err(CommandError::Gateway)?;
    let refusal = cache.take_refusal();
    let saved = cache.retained_catalogue_progress(scope.receiver(), scope.origin(), scope.stream());
    online::write_catalogue(&attempt, saved, refusal, output)
}

#[cfg(test)]
#[path = "../../../tests/composition/read_only_online.rs"]
mod tests;

fn open_cache(
    path: &Path,
    policy: CachePolicy,
    output: &mut dyn Write,
) -> Result<ReadOnlyCache, CommandError> {
    match ReadOnlyCache::open(path, policy, Arc::new(SystemClock)) {
        Ok(cache) => {
            #[cfg(test)]
            let mut cache = cache;
            #[cfg(test)]
            BEFORE_SAVED_REFRESH.with(|hook| {
                if let Some(proceed) = hook.borrow_mut().take() {
                    cache.before_projection_refresh(proceed);
                }
            });
            Ok(cache)
        }
        Err(error) => {
            online::write(
                json!({"successful":false,"connectionCheck":"performed","cacheRefusal":online::cache_failure(&error)}),
                output,
            )?;
            Err(CommandError::OnlineRefused)
        }
    }
}
