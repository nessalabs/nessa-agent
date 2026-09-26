//! Ask an agent to delete its own record of a session nothing runs any more.
//!
//! ```text
//! ProcessFactory -> initialize -> delete advertised? --no--> NotAdvertised
//!                                     | yes
//!                                     v
//!                              session/delete --accepted--> Acknowledged
//!                                     | refused
//!                                     v
//!         list advertised, and read in full without naming it? --yes--> NotListed
//!                                     | no
//!                                     v
//!                              the refusal, a failure
//! ```
//! Arrows are the order of the exchange on one connection of its own, which
//! then stops its process.
//!
//! The delete is sent first, whenever the agent advertises it, and an
//! accepted delete depends on nothing else: only the agent knows whether it
//! still has the session, and an agent may leave a session out of its list —
//! Claude's lists none with no titled prompt, so a conversation of images
//! alone is never listed.
//!
//! The list is read only after a refusal, to interpret it. A refusal of a
//! session the agent does not list is what an agent says of a session it no
//! longer has — a deletion interrupted after the agent deleted and before that
//! was written down — so it settles as [`AcpSessionDeletion::NotListed`], and a
//! deletion is never refused forever for a session that is gone. A refusal the
//! list does not explain stays the refusal, a failure asked again later: the
//! list names the session, or could not be read in full — refused, past its
//! budget, a page without `sessions`, an entry without a string `sessionId`, a
//! `nextCursor` that is neither a string nor null or repeats one followed, more
//! than [`MAX_LIST_PAGES`] pages, or a page past [`MAX_LISTING_FRAME_BYTES`]
//! or [`MAX_LISTING_ITEMS`]. Nothing
//! is claimed about a session the list could not speak for, so no such
//! refusal settles. Only the agent's own error answer counts as a refusal; a
//! lost connection or a budget running out is a failure. Nothing reads an
//! error's text, and the list's own failure is never what is returned.
//!
//! The delete has one `startup_timeout`; the list, for the workspace this
//! binding runs in and paged by `nextCursor`, one more, however many pages it
//! has (`the_whole_listing_shares_one_budget`). Every budget is a moment on
//! `AcpConfig::clock`. An agent may answer with its whole list in one frame —
//! Claude's does — so a list is read with bounds of its own, far past the rest
//! of the protocol's: values only while the list is read, bytes for the whole
//! connection, since a stream's byte bound is fixed (the cost is stated where
//! the connection is made).
//!
//! This is the shared exchange only. What an acknowledged delete means is
//! decided by each agent's binding, in that binding's own module, which calls
//! this and names the result. The session is never loaded or resumed:
//! `session/delete` is the only request that names it. The process is launched
//! exactly as an opening launches one, and stopped within the shutdown budgets,
//! even when the caller stops waiting; one whose stop is not confirmed goes to
//! the supervisor `ProcessCleanup` hands abandoned processes to. The whole
//! bound is `AcpConfig::session_deletion_limit`.
//! Regressions: `tests/infrastructure/acp/contracts/deletion.rs`.
use super::{binding::ProcessFactory, cleanup::ProcessCleanup, AcpConfig};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::providers::ProviderCleanup;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::{
    executions::worker::{check_initialize, initialize_params, provider_failure},
    profile::AcpProfile,
};
use crate::infrastructure::{
    clock::ClockInstant,
    json_rpc::{self, Reader, RpcId},
    process::ProcessScope,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    process::ChildStdout,
    sync::{oneshot, Notify},
    time::timeout,
};

/// What the exchange confirmed, before an agent's binding says what it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcpSessionDeletion {
    /// The agent answered `session/delete` with success.
    Acknowledged,
    /// The agent refused to delete the session, and its list, for the
    /// workspace, read in full after that, does not name it: what an agent
    /// says of a session it no longer has.
    NotListed,
    /// The agent did not advertise `sessionCapabilities.delete`; nothing was
    /// asked.
    NotAdvertised,
}

/// Launch the agent, ask it to delete `session` if it offers that, and stop
/// it. The contract is
/// [`ProviderSessionDeleter::delete_session`](crate::application::agent_execution::providers::ProviderSessionDeleter::delete_session)'s.
///
/// The exchange and the process it launched are owned by a task of their own,
/// not by the caller's future: a caller that stops waiting ends the exchange
/// where it is, and the task still stops the process and releases what its
/// launch made — confirmed, or handed to [`ProcessCleanup`], whose owner keeps
/// retrying — exactly as when the answer is waited for
/// (`a_deletion_abandoned_mid_exchange_still_stops_its_agent_and_releases_its_home`).
pub(crate) async fn delete_session<P: AcpProfile + Clone>(
    process: ProcessFactory,
    config: AcpConfig,
    profile: &P,
    session: &ExecutionSessionId,
    cleanups: &DeletionCleanups,
) -> Result<AcpSessionDeletion, AgentError> {
    config.validate()?;
    // Counted from before anything is admitted or launched, and handed to
    // whichever cleanup owner ends up holding what the launch made: the count
    // ends only once that is confirmed released (ADR 221).
    let outstanding = cleanups.begin();
    // The executable is held in use for this launch exactly as an opening
    // holds it, and let go only once what the launch made is confirmed gone;
    // every cleanup owner below keeps retrying after it is dropped.
    let executable_use = match config.executable.admit() {
        Ok(guard) => guard,
        Err(failure) => {
            let (error, owner) = failure.into_parts();
            if let Some(owner) = owner {
                let cleanup = ProcessCleanup::retaining_failed_admission(config.clone(), owner)
                    .until_released(outstanding);
                if !cleanup.retry_cleanup().await.is_confirmed() {
                    tracing::warn!("a failed executable admission is retained for cleanup");
                }
            }
            return Err(AgentError::Configuration(format!(
                "executable use admission failed: {error}"
            )));
        }
    };
    let mut scope = match process() {
        Ok(scope) => scope,
        Err(failure) => {
            let (cause, recovery) = failure.into_parts();
            let cleanup = match recovery {
                // A private directory made for a launch that failed.
                Some(directory) => Some(ProcessCleanup::retaining_directory(
                    config.clone(),
                    directory,
                    executable_use,
                )),
                None => {
                    let mut executable_use = executable_use;
                    match executable_use.release() {
                        Ok(()) => None,
                        Err(_) => Some(ProcessCleanup::retaining_use(
                            config.clone(),
                            executable_use,
                        )),
                    }
                }
            };
            if let Some(cleanup) = cleanup {
                let cleanup = cleanup.until_released(outstanding);
                if !cleanup.retry_cleanup().await.is_confirmed() {
                    tracing::warn!("a failed launch's resources are retained for cleanup");
                }
            }
            return Err(cause);
        }
    };
    let recovery = ProcessCleanup::new(config.clone(), executable_use).until_released(outstanding);
    let (reply, answer) = oneshot::channel();
    let profile = profile.clone();
    let session = session.clone();
    tokio::spawn(async move {
        let mut reply = reply;
        let outcome = tokio::select! {
            outcome = exchange(&mut scope, &config, &profile, &session) => Some(outcome),
            // Nobody is waiting any more: the exchange ends here, and the
            // process is still stopped below.
            () = reply.closed() => None,
        };
        match scope
            .cleanup(config.shutdown_grace, config.kill_timeout)
            .await
        {
            Ok(outcome) => {
                if !recovery.confirm_physical(outcome).await.is_confirmed() {
                    tracing::warn!(
                        "a session-deletion connection's executable use is retained for cleanup"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "a session-deletion connection's process is retained for cleanup");
                recovery.retain(scope).await;
            }
        }
        if let Some(outcome) = outcome {
            let _ = reply.send(outcome);
        }
    });
    // A task that ended without answering had no answer to give.
    answer.await.unwrap_or(Err(AgentError::Closed))
}

async fn exchange<P: AcpProfile>(
    scope: &mut ProcessScope,
    config: &AcpConfig,
    profile: &P,
    session: &ExecutionSessionId,
) -> Result<AcpSessionDeletion, AgentError> {
    let stdout = scope
        .stdout
        .take()
        .ok_or_else(|| AgentError::Transport("provider stdout is not available".into()))?;
    let mut connection = Connection {
        scope,
        // Everything this connection reads is small but the list, which an
        // agent may send whole in one frame. Its values are bounded as the
        // protocol's everywhere else until the list, the last thing it reads,
        // is read (`Connection::lists`). Its bytes cannot be: the framer's
        // bound is fixed for the stream, so any frame here may be up to
        // `MAX_LISTING_FRAME_BYTES` — the most `AcpConfig` allows any frame,
        // so never less than the configured bound. The cost is that one
        // frame's buffer, on a connection that exists only for a delete and
        // lives at most `AcpConfig::session_deletion_limit`; the buffer grows
        // only as bytes arrive, so an agent that sends small frames costs
        // nothing more.
        reader: Reader::new(stdout, MAX_LISTING_FRAME_BYTES),
        config,
        sequence: 0,
    };
    // The same two budgets an opening has: the launch's, which ends when the
    // agent first answers, and the protocol's after it.
    let init = connection
        .request(
            "initialize",
            // Deleting a session asks nothing of anybody.
            initialize_params(false),
            config.clock.now() + config.launch_timeout,
        )
        .await?;
    check_initialize(profile, &init)?;
    if !advertises(&init, "delete") {
        return Ok(AcpSessionDeletion::NotAdvertised);
    }
    let refusal = match connection
        .request(
            "session/delete",
            json!({"sessionId": session.as_str()}),
            config.clock.now() + config.startup_timeout,
        )
        .await
    {
        Ok(_) => return Ok(AcpSessionDeletion::Acknowledged),
        Err(refusal @ AgentError::Provider { .. }) if advertises(&init, "list") => refusal,
        Err(error) => return Err(error),
    };
    // Only a refusal is read against the list, and only a list read in full
    // that does not name the session changes what it means.
    let deadline = config.clock.now() + config.startup_timeout;
    match connection
        .lists(session, json!(config.workspace), deadline)
        .await
    {
        Ok(false) => Ok(AcpSessionDeletion::NotListed),
        Ok(true) | Err(_) => Err(refusal),
    }
}

/// The deletions one binding has started whose process is not yet stopped
/// and released. Shared by every clone of the binding.
///
/// A deletion abandoned by its caller keeps running on a task of its own
/// until its process is stopped; a host that is about to end its runtime waits
/// for [`Self::settled`] first, since a runtime that ends drops that task
/// mid-cleanup and what the process's launch made — a private home, say — is
/// left behind (`a_binding_settles_once_an_abandoned_deletion_has_released_its_home`).
#[derive(Clone, Default)]
pub(crate) struct DeletionCleanups(Arc<Cleanups>);
#[derive(Default)]
struct Cleanups {
    outstanding: AtomicUsize,
    settled: Notify,
}
/// One deletion counted as outstanding until this is dropped.
struct Outstanding(Arc<Cleanups>);
impl Drop for Outstanding {
    fn drop(&mut self) {
        self.0.outstanding.fetch_sub(1, Ordering::SeqCst);
        self.0.settled.notify_waiters();
    }
}
impl DeletionCleanups {
    fn begin(&self) -> Outstanding {
        self.0.outstanding.fetch_add(1, Ordering::SeqCst);
        Outstanding(self.0.clone())
    }
    /// Whether a deletion this binding started is still admitting, launching,
    /// asking, or releasing what its launch made, including when a retrying
    /// cleanup owner holds it. Still true after [`Self::settled`] gave up
    /// waiting on one.
    pub(crate) fn outstanding(&self) -> bool {
        self.0.outstanding.load(Ordering::SeqCst) > 0
    }

    /// Until no deletion this binding started still has a process to stop,
    /// or for at most the time stopping one takes with `config`'s budgets —
    /// `shutdown_grace` + 4 × `kill_timeout` — after which what is still
    /// outstanding has already been handed to the cleanup supervisor or is
    /// still asking its agent.
    pub(crate) async fn settled(&self, config: &AcpConfig) {
        let budget = config.shutdown_grace + config.kill_timeout * 4;
        let _ = timeout(budget, async {
            loop {
                let settled = self.0.settled.notified();
                tokio::pin!(settled);
                settled.as_mut().enable();
                if self.0.outstanding.load(Ordering::SeqCst) == 0 {
                    return;
                }
                settled.await;
            }
        })
        .await;
    }
}

/// Most pages one listing reads before it gives up as a protocol failure.
pub(crate) const MAX_LIST_PAGES: usize = 64;

/// The largest frame this connection reads: an agent may send its whole list
/// of sessions in one — Claude's does, at about 213 bytes a session — and the
/// rest of the protocol's bound, about 5,000 of those, is one busy person's
/// store. 16 MiB is the ceiling `AcpConfig` allows any frame, about 78,000
/// sessions, read once, only for a delete.
pub(crate) const MAX_LISTING_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The most values and keys one frame on this connection may hold. A Claude
/// session is nine; this keeps pace with [`MAX_LISTING_FRAME_BYTES`] for such
/// entries while still bounding what tiny values could allocate.
pub(crate) const MAX_LISTING_ITEMS: usize = 1 << 20;

/// Whether `initialize` advertised `agentCapabilities.sessionCapabilities.<name>`.
fn advertises(init: &Value, name: &str) -> bool {
    init.pointer(&format!("/agentCapabilities/sessionCapabilities/{name}"))
        .is_some_and(Value::is_object)
}

struct Connection<'a> {
    scope: &'a mut ProcessScope,
    reader: Reader<ChildStdout>,
    config: &'a AcpConfig,
    sequence: i64,
}
impl Connection<'_> {
    /// Send one request and wait, until `deadline`, for its answer. Requests
    /// the agent makes meanwhile are refused as unsupported, since this
    /// connection runs nothing they could be about; notifications are read
    /// past.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: ClockInstant,
    ) -> Result<Value, AgentError> {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json_rpc::request(id, method, params), deadline)
            .await?;
        loop {
            let message = tokio::select! { biased;
                () = self.config.clock.sleep_until(deadline) => return Err(AgentError::Deadline),
                message = self.reader.next() => message?,
            };
            if message.method.is_some() {
                if let Some(request) = &message.id {
                    self.send(json_rpc::unsupported(request), deadline).await?;
                }
                continue;
            }
            if message.id != Some(RpcId::Number(id)) {
                return Err(json_rpc::protocol("unexpected response"));
            }
            if let Some(error) = message.error {
                return Err(provider_failure(method, error));
            }
            return message
                .result
                .ok_or_else(|| json_rpc::protocol("missing response result"));
        }
    }
    /// Whether `session/list`, filtered to `cwd`, names `session`,
    /// following `nextCursor` for at most [`MAX_LIST_PAGES`] pages, all before
    /// `deadline`.
    ///
    /// # Errors
    /// The agent's refusal, a budget running out, a page too large to read,
    /// or a response that is not a list of sessions: a repeated cursor, too
    /// many pages, or a page without `sessions`. None of these is taken to
    /// mean the session is gone.
    async fn lists(
        &mut self,
        session: &ExecutionSessionId,
        cwd: Value,
        deadline: ClockInstant,
    ) -> Result<bool, AgentError> {
        // Nothing is read after the list, so the bound is never lowered again.
        self.reader.allow_json_items(MAX_LISTING_ITEMS);
        self.pages(session, cwd, deadline).await
    }
    /// [`Self::lists`], page by page. An entry without a string `sessionId`
    /// is not a list this can read, so it is a protocol failure, never taken
    /// to mean the session is not named
    /// (`a_refused_delete_the_list_cannot_explain_stays_the_refusal`).
    async fn pages(
        &mut self,
        session: &ExecutionSessionId,
        cwd: Value,
        deadline: ClockInstant,
    ) -> Result<bool, AgentError> {
        let mut cursor: Option<String> = None;
        // Every cursor followed so far: a list that comes back to one is a
        // loop, whatever came between (at most `MAX_LIST_PAGES` of them).
        let mut followed = std::collections::HashSet::new();
        for _ in 0..MAX_LIST_PAGES {
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), cwd.clone());
            if let Some(cursor) = &cursor {
                params.insert("cursor".into(), json!(cursor));
            }
            let page = self
                .request("session/list", Value::Object(params), deadline)
                .await?;
            let sessions = page
                .get("sessions")
                .and_then(Value::as_array)
                .ok_or_else(|| json_rpc::protocol("session list without sessions"))?;
            for listed in sessions {
                let named = listed
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| json_rpc::protocol("session list entry without a sessionId"))?;
                if named == session.as_str() {
                    return Ok(true);
                }
            }
            match page.get("nextCursor") {
                None | Some(Value::Null) => return Ok(false),
                Some(Value::String(next)) if followed.insert(next.clone()) => {
                    cursor = Some(next.clone());
                }
                Some(_) => return Err(json_rpc::protocol("session list cursor repeats")),
            }
        }
        Err(json_rpc::protocol("session list exceeds its page bound"))
    }
    async fn send(&mut self, value: Value, deadline: ClockInstant) -> Result<(), AgentError> {
        let frame = json_rpc::encode(value, self.config.max_frame_bytes)?;
        let stdin = self.scope.stdin.as_mut().ok_or(AgentError::Closed)?;
        json_rpc::send_encoded(
            &*self.config.clock,
            stdin,
            &frame,
            json_rpc::write_allowance(frame.len()),
            Some(deadline),
        )
        .await
    }
}
