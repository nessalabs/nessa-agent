//! The agent's environment tools (issue #700): `environments_list`, where it
//! could work and what it may do there, and `run`, one bounded command on an
//! SSH host under a command lease (ADR 252 rows L14, L15).
//!
//! ```text
//! run(conversation, call) ──▶ the running turn? ── no ──▶ NoTurn, nothing recorded
//!   ──▶ the command's policy, sandbox and budget ── refused ──▶ CommandRefused
//!   ──▶ CommandEnvironment::grant ── refused ──▶ CommandRefused
//!   ──▶ CommandIssued (the turn's person, this call) ── unsaved ──▶ the hold dropped
//!   ──▶ CommandHold::run(stop) ──▶ CommandEnded{exit, output, cleanup}
//! ```
//!
//! Arrows are steps, in order; every refusal and every end is a record in the
//! conversation's stream, under its Live agent lease, naming who asked. The
//! command runs for the person whose turn asked for it: that turn's actor,
//! with the tool call as its request. A command is bounded by its tool call:
//! the caller going away, or the agent's lease ending, stops it (row L15),
//! and what it printed is kept as evidence under its record's bound.
use super::{ConversationError, ConversationService, LiveConversation};
use crate::conversation::application::{CommandResult, Environment};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    permissions::ActionContext,
    sessions::{CurrentLease, LeaseRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::executions::InvocationStage;
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandOutput, CommandRefusal, CommandTerms, CommandWork, EnvironmentRef, Lease,
    LeaseEndCause, LeaseId, LeasePhase, SandboxProfile, SandboxProfiles, SshDestination,
};
use std::sync::Arc;
use tokio::sync::watch;

/// The environment named `here`: this gateway's own machine.
pub const HERE: &str = "here";

/// One environment the agent could work in, as `environments_list` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentListing {
    /// `here`, or the SSH destination the configuration names.
    pub name: String,
    /// Whether it is this gateway's own machine.
    pub here: bool,
    /// `Some(true)` with a connection to it open now; `None` when not known
    /// without reaching it. Nothing is reached to answer.
    pub reachable: Option<bool>,
    /// Whether the agent may run commands there with `run`.
    pub commands: bool,
    /// Whether this conversation's agent runs there.
    pub current: bool,
}

/// One `run` call.
#[derive(Clone, Debug)]
pub struct CommandCall {
    /// Where: an SSH destination `environments_list` named.
    pub environment: String,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub timeout_ms: u64,
    /// The sandbox asked for; a command can be given only `none`.
    pub sandbox: SandboxProfile,
    /// The tool call's identity, the request the command runs for.
    pub call: String,
}

/// What a `run` came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandAnswer {
    /// It was refused, with its reason; nothing ran.
    Refused(CommandRefusal),
    /// It ran, under `lease`. `recorded` is false when its end could not
    /// be saved, even once more: the end stays retained and goes with the
    /// conversation's next save, but the records do not hold it yet.
    Ran {
        lease: LeaseId,
        result: CommandResult,
        recorded: bool,
    },
}

/// Why a `run` was not even decided: nothing was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandCallError {
    /// The command is not one a command lease can hold.
    Invalid(String),
    /// No turn of this conversation is running here, so nobody it could run
    /// for, or no agent lease it could run under.
    NoTurn,
    /// Its record could not be written.
    Unrecorded,
    /// This gateway grants no commands.
    NotConfigured,
    /// Its caller went away before anything was asked of an environment.
    Cancelled,
}

impl ConversationService {
    /// Whether this gateway grants the agent's environment tools at all.
    pub fn environment_tools_enabled(&self) -> bool {
        self.inner.environment.command_policy().is_some()
    }

    /// Where conversation `id`'s agent could work: here and every configured
    /// host, what is known of reaching each without reaching it, and whether
    /// commands are granted there.
    pub async fn list_environments(
        &self,
        id: &ConversationId,
    ) -> Result<Vec<EnvironmentListing>, ConversationError> {
        let environments = &self.inner.environment;
        let current = environments.placement(id).await?;
        let policy = environments.command_policy();
        let mut listed = vec![EnvironmentListing {
            name: HERE.into(),
            here: true,
            reachable: Some(true),
            commands: false,
            current: current.is_none(),
        }];
        for (name, environment) in environments.hosts() {
            let commands = environment.commands();
            listed.push(EnvironmentListing {
                name: name.clone(),
                here: false,
                reachable: commands.and_then(|commands| commands.reachable()),
                commands: commands.is_some() && policy.is_some_and(|policy| policy.grants(name)),
                current: current.as_deref() == Some(name.as_str()),
            });
        }
        Ok(listed)
    }

    /// Run `call` for conversation `id`'s running turn, once, and record
    /// every step of it. `stop`, once true, stops it as its caller gone.
    ///
    /// # Errors
    /// [`CommandCallError`]: nothing was decided, and nothing recorded,
    /// except [`CommandCallError::Unrecorded`], whose command did not run.
    pub async fn run_command(
        &self,
        id: &ConversationId,
        call: CommandCall,
        stop: watch::Receiver<bool>,
    ) -> Result<CommandAnswer, CommandCallError> {
        let policy = self
            .inner
            .environment
            .command_policy()
            .ok_or(CommandCallError::NotConfigured)?
            .clone();
        let work = CommandWork::new(call.argv.clone(), call.cwd.clone(), call.timeout_ms)
            .map_err(|error| CommandCallError::Invalid(error.to_string()))?;
        let live = self.live(id).await.ok_or(CommandCallError::NoTurn)?;
        let manager = live.agent.session_manager();
        let snapshot = manager.snapshot().await.ok_or(CommandCallError::NoTurn)?;
        let (parent, actor) =
            running_turn(&snapshot, &call.call).ok_or(CommandCallError::NoTurn)?;
        let lease = LeaseId::new(uuid::Uuid::new_v4().to_string())
            .expect("a UUID is a portable lease identity");
        // A name that is no host is not recorded as one: nothing names it.
        let host = match call.environment.as_str() {
            HERE => None,
            named => Some(SshDestination::new(named).map_err(|_| {
                CommandCallError::Invalid(format!("{named:?} names no environment"))
            })?),
        };
        // A caller already gone is asked nothing.
        if *stop.borrow() {
            return Err(CommandCallError::Cancelled);
        }
        let terms = CommandTerms {
            environment: host
                .clone()
                .map_or(EnvironmentRef::Here, EnvironmentRef::Ssh),
            command: work.clone(),
            sandbox: call.sandbox,
        };
        let refused = Refused {
            lease: &lease,
            parent: &parent,
            terms: &terms,
            actor: &actor,
        };
        // Where: a host this gateway names, granted, whose environment runs
        // commands. Here runs none: its commands are the agent's own shell's.
        let environment: Option<Arc<dyn Environment>> = host
            .as_ref()
            .and_then(|host| self.inner.environment.hosts().get(host.as_str()).cloned());
        let Some(environment) = environment else {
            let refusal = if call.environment == HERE {
                CommandRefusal::CommandsUnavailable
            } else {
                CommandRefusal::EnvironmentNotGranted
            };
            return self.refuse(&live, &refused, refusal).await;
        };
        if let Err(refusal) = policy.admit(&call.environment, work.program()) {
            return self.refuse(&live, &refused, refusal).await;
        }
        if let Err(refusal) = SandboxProfiles::UNENCLOSED.admit(call.sandbox) {
            return self
                .refuse(&live, &refused, CommandRefusal::Environment(refusal))
                .await;
        }
        if !has_room(snapshot.lease.as_ref(), &parent) {
            return self
                .refuse(&live, &refused, CommandRefusal::BudgetExceeded)
                .await;
        }
        let Some(commands) = environment.commands() else {
            return self
                .refuse(&live, &refused, CommandRefusal::CommandsUnavailable)
                .await;
        };
        let hold = match commands.grant(&lease, &work).await {
            Ok(hold) => hold,
            Err(refusal) => return self.refuse(&live, &refused, refusal).await,
        };
        // Recorded under the lock as granted, or refused if the lease filled
        // or ended meanwhile: either way the hold goes when not run.
        let issued = LeaseRecord::CommandIssued {
            lease: lease.clone(),
            parent: parent.clone(),
            terms: terms.clone(),
            actor: actor.clone(),
        };
        let parent_for_check = parent.clone();
        let committed = manager
            .record_lease(move |current| {
                if has_room(current, &parent_for_check) {
                    (vec![issued], Issuance::Issued)
                } else if live_parent(current, &parent_for_check) {
                    (Vec::new(), Issuance::Full)
                } else {
                    (Vec::new(), Issuance::NotLive)
                }
            })
            .await;
        match committed {
            Ok(commit) if commit.decided == Issuance::Issued && commit.saved.is_ok() => {}
            Ok(commit) if commit.decided == Issuance::Full => {
                drop(hold);
                return self
                    .refuse(&live, &refused, CommandRefusal::BudgetExceeded)
                    .await;
            }
            // Its turn's lease ended while the host was asked: nobody to run
            // it for, as if no turn had been running.
            Ok(commit) if commit.decided == Issuance::NotLive => {
                return Err(CommandCallError::NoTurn);
            }
            Ok(commit) => {
                // Folded and kept for a later save: its end is kept beside
                // it, so the lease never counts a command that never ran.
                drop(hold);
                tracing::error!(error = ?commit.saved, conversation_id = %id, "a command lease could not be saved; it does not run");
                let ended = LeaseRecord::CommandEnded {
                    lease: lease.clone(),
                    parent: parent.clone(),
                    exit: CommandExit::NotStarted,
                    output: CommandOutput::new(0, 0, 0, "", ""),
                    cleanup: None,
                };
                let _ = manager.record_lease(move |_| (vec![ended], ())).await;
                return Err(CommandCallError::Unrecorded);
            }
            Err(error) => {
                tracing::error!(?error, conversation_id = %id, "a command lease could not be recorded; it does not run");
                return Err(CommandCallError::Unrecorded);
            }
        }
        // A caller gone during the grant stops it before it starts.
        let gone = *stop.borrow();
        let (stopping, stopped) = watch::channel(gone.then_some(LeaseEndCause::Closed));
        let mut caller = stop;
        let mut ran = hold.run(stopped);
        let result = loop {
            tokio::select! {
                result = &mut ran => break result,
                // The caller gone: the command is stopped as closed, and its
                // answer still waited for and recorded.
                changed = caller.changed(), if stopping.borrow().is_none() => {
                    if changed.is_err() || *caller.borrow() {
                        stopping.send_replace(Some(LeaseEndCause::Closed));
                    }
                }
            }
        };
        let result = CommandResult {
            exit: self.stop_cause(&live, &parent, result.exit).await,
            ..result
        };
        let ended = LeaseRecord::CommandEnded {
            lease: lease.clone(),
            parent: parent.clone(),
            exit: result.exit,
            output: CommandOutput::new(
                result.stdout.len() as u64,
                result.stderr.len() as u64,
                result.dropped_bytes,
                &String::from_utf8_lossy(&result.stdout),
                &String::from_utf8_lossy(&result.stderr),
            ),
            cleanup: result.cleanup,
        };
        // Recorded while its parent is still the conversation's latest lease;
        // once a later agent lease replaced it, the parent's own end accounts
        // for the command, and its exit is only logged.
        let parent_for_end = parent.clone();
        let committed = manager
            .record_lease(move |current| {
                if current
                    .and_then(CurrentLease::held)
                    .is_some_and(|held| held.id() == &parent_for_end)
                {
                    (vec![ended], ())
                } else {
                    (Vec::new(), ())
                }
            })
            .await;
        // An end the records do not hold is not answered as if they did: one
        // more save is tried, then the answer says it is unsaved.
        let recorded = match &committed {
            Ok(commit) if commit.saved.is_ok() => true,
            Ok(_) => manager.save_retained_lease_records().await.is_ok(),
            Err(_) => false,
        };
        if !recorded {
            tracing::error!(conversation_id = %id, lease = lease.as_str(), exit = ?result.exit, "a command's end could not be recorded");
        }
        Ok(CommandAnswer::Ran {
            lease,
            result,
            recorded,
        })
    }

    /// Record `refused` as refused for `refusal`, and answer so.
    async fn refuse(
        &self,
        live: &LiveConversation,
        refused: &Refused<'_>,
        refusal: CommandRefusal,
    ) -> Result<CommandAnswer, CommandCallError> {
        self.record_command(
            live,
            refused.parent,
            LeaseRecord::CommandRefused {
                lease: refused.lease.clone(),
                parent: refused.parent.clone(),
                terms: refused.terms.clone(),
                refusal,
                actor: refused.actor.clone(),
            },
        )
        .await
        .map(|()| CommandAnswer::Refused(refusal))
    }

    /// Record one command record for `live`'s conversation, while its parent
    /// is still the conversation's latest lease; [`CommandCallError::NoTurn`]
    /// once another replaced it.
    async fn record_command(
        &self,
        live: &LiveConversation,
        parent: &LeaseId,
        record: LeaseRecord,
    ) -> Result<(), CommandCallError> {
        let parent = parent.clone();
        let committed = live
            .agent
            .session_manager()
            .record_lease(move |current| {
                if current
                    .and_then(CurrentLease::held)
                    .is_some_and(|held| held.id() == &parent)
                {
                    (vec![record], true)
                } else {
                    (Vec::new(), false)
                }
            })
            .await;
        match committed {
            Ok(commit) if !commit.decided => Err(CommandCallError::NoTurn),
            Ok(commit) if commit.saved.is_ok() => Ok(()),
            _ => Err(CommandCallError::Unrecorded),
        }
    }

    /// `exit` with a stop's cause made the agent lease's own when that lease
    /// is ending: the command stopped because its parent did (row L15).
    async fn stop_cause(
        &self,
        live: &LiveConversation,
        parent: &LeaseId,
        exit: CommandExit,
    ) -> CommandExit {
        let CommandExit::Stopped { .. } = exit else {
            return exit;
        };
        let ending = live
            .agent
            .session_manager()
            .snapshot()
            .await
            .and_then(|snapshot| snapshot.lease)
            .and_then(|lease| lease.held().cloned())
            .filter(|lease| lease.id() == parent)
            .and_then(|lease| match lease.phase() {
                LeasePhase::Ending { cause } => Some(cause),
                LeasePhase::Ended { cause, .. } | LeasePhase::Interrupted { cause, .. } => {
                    Some(cause)
                }
                LeasePhase::Live => None,
            });
        match ending {
            Some(cause) => CommandExit::Stopped { cause },
            None => exit,
        }
    }

    /// Conversation `id`, open here now; `None` when it is not.
    async fn live(&self, id: &ConversationId) -> Option<Arc<LiveConversation>> {
        let slot = self.inner.conversations.lock().await.get(id).cloned()?;
        slot.value.get()?.as_ref().ok().cloned()
    }
}

/// What a refused command's record names.
struct Refused<'a> {
    lease: &'a LeaseId,
    parent: &'a LeaseId,
    terms: &'a CommandTerms,
    actor: &'a ActionContext,
}

/// The conversation's Live agent lease and the person the running turn is
/// for, with `call` as their request; `None` with no turn running or no
/// Live lease to run under.
fn running_turn(snapshot: &SessionSnapshot, call: &str) -> Option<(LeaseId, ActionContext)> {
    let parent = snapshot
        .lease
        .as_ref()
        .and_then(CurrentLease::held)
        .filter(|lease| lease.phase() == LeasePhase::Live)?
        .id()
        .clone();
    let turn = snapshot.invocations.iter().rev().find(|record| {
        record.result.is_none()
            && record
                .scheduling
                .last()
                .is_some_and(|event| event.stage == InvocationStage::Running)
    })?;
    let actor = &turn.actor;
    let actor = ActionContext::new(actor.principal_id(), actor.surface_id(), call).ok()?;
    Some((parent, actor))
}

/// What issuing a granted command came to, under the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Issuance {
    Issued,
    /// Its parent is Live, with as many commands as it holds.
    Full,
    /// Its parent is no longer Live, or no longer the latest lease.
    NotLive,
}

/// Whether `parent` is the conversation's latest lease, and Live.
fn live_parent(current: Option<&CurrentLease>, parent: &LeaseId) -> bool {
    current
        .and_then(CurrentLease::held)
        .is_some_and(|lease| lease.id() == parent && lease.phase() == LeasePhase::Live)
}

/// Whether `parent`, the conversation's latest lease, is Live with room for
/// another command.
fn has_room(current: Option<&CurrentLease>, parent: &LeaseId) -> bool {
    current
        .and_then(CurrentLease::held)
        .is_some_and(|lease: &Lease| {
            lease.id() == parent
                && lease.phase() == LeasePhase::Live
                && lease.commands().len() < Lease::MAX_LIVE_COMMANDS
        })
}
