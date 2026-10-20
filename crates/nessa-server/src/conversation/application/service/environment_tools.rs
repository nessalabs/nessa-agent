//! The agent's environment tools (issue #700): `environments_list`, where it
//! could work and what it may do there, and `run`, one bounded command on an
//! SSH host under a command lease (ADR 252 rows L14, L15).
//!
//! ```text
//! run(conversation, call) ──▶ the running turn? ── no ──▶ NoTurn, nothing recorded
//!   ──▶ a Drive grant's turn: its tool policy allows `run`? ── no ──▶ NotAllowed
//!   ──▶ admit_command(turn's initiator, host, program), sandbox and budget
//!       ── refused ──▶ CommandRefused
//!   ──▶ CommandEnvironment::grant ── refused ──▶ CommandRefused
//!   ──▶ the terms granted within those asked? ── no ──▶ the hold dropped,
//!       CommandRefused(environment not granted)
//!   ──▶ CommandIssued with the terms granted (the turn's person, this call)
//!       ── unsaved ──▶ the hold dropped
//!   ──▶ CommandHold::run(stop) ──▶ CommandEnded{exit, output, cleanup}
//! ```
//!
//! Arrows are steps, in order; every refusal and every end is a record in the
//! conversation's stream, under its Live agent lease, naming who asked. The
//! command runs for the person whose turn asked for it: that turn's actor,
//! with the tool call as its request. A command is bounded by its tool call:
//! the caller going away, or the agent's lease ending, stops it (row L15),
//! and what it printed is kept as evidence under its record's bound.
//!
//! Where a command may go, and which program it may run, is the command
//! policy's (`CommandPolicyEvaluator`, ADR 803 step 4), asked for the turn's
//! initiator: the owner uses every host the gateway grants commands on, a
//! Drive grant's turn only those its policy at the turn's revision names, and
//! none when it names none. `environments_list` asks the same policy, so a
//! grant's turn is shown only the hosts it may use.
//!
//! What is recorded issued is what the environment granted, never what was
//! asked (row L1): its timeout may be shorter than asked, under the
//! environment owner's own policy. An answer granting more — a longer
//! timeout, another sandbox — is never run.
use super::{ConversationError, ConversationService, LiveConversation};
use crate::conversation::application::drive_tools::{
    decision, AdmittedPolicies, DriveToolAuditRecord, DriveTools,
};
use crate::conversation::application::{
    admit_command, environment::GrantedTerms, CommandInitiator, CommandPolicyEvaluator,
    CommandQuestion, CommandResult, Environment, ToolDecision,
};
use crate::conversation::domain::{ToolCall, ToolPolicy, ToolSource};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    permissions::ActionContext,
    sessions::{CurrentLease, InvocationRecord, LeaseRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, Initiation, Initiator, InvocationStage,
};
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandOutput, CommandRefusal, CommandTerms, CommandWork, EnvironmentRef, Lease,
    LeaseEndCause, LeaseId, LeasePhase, LeaseRefusal, SandboxProfile, SandboxProfiles,
};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

/// The environment named `here`: this gateway's own machine.
pub const HERE: &str = EnvironmentRef::HERE;

/// One environment the agent could work in, as `environments_list` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentListing {
    /// `here`, the SSH destination the configuration names, or a connected
    /// paired environment's `peer:<key>`.
    pub name: String,
    /// Whether it is this gateway's own machine.
    pub here: bool,
    /// `Some(true)` with a connection to it open now; `None` when not known
    /// without reaching it. Nothing is reached to answer.
    pub reachable: Option<bool>,
    /// Whether the running turn's initiator may run commands there with
    /// `run`, as the command policy answers.
    pub commands: bool,
    /// Whether this conversation's agent runs there.
    pub current: bool,
}

/// One `run` call.
#[derive(Clone, Debug)]
pub struct CommandCall {
    /// Where: an SSH host or paired environment `environments_list` named.
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
    /// The running turn's Drive grant does not allow `run`, or its decision
    /// could not be recorded in the Drive tool audit, with why. No lease
    /// record was written.
    NotAllowed(String),
}

impl ConversationService {
    /// Whether this gateway grants the agent's environment tools at all.
    pub fn environment_tools_enabled(&self) -> bool {
        self.inner.environment.command_policy().is_some()
    }

    /// Where conversation `id`'s agent could work, for its running turn's
    /// initiator: here, and each configured host and connected paired
    /// environment the command policy lets that initiator use, with what is
    /// known of reaching each without reaching it and whether it takes
    /// commands. The owner is shown every one, commands or not; a Drive
    /// grant's turn only the hosts it may use, and the one the conversation
    /// runs on. With no turn running, or a grant whose policy is no longer
    /// kept, nobody may use any host, and only here and where it runs are
    /// shown.
    pub async fn list_environments(
        &self,
        id: &ConversationId,
    ) -> Result<Vec<EnvironmentListing>, ConversationError> {
        let environments = &self.inner.environment;
        let current = environments.runs_on(id).await?;
        let initiator = match self.live(id).await {
            Some(live) => {
                let snapshot = live.agent.session_manager().snapshot().await;
                snapshot
                    .as_ref()
                    .and_then(running_invocation)
                    .and_then(|turn| TurnInitiator::of(&live.admitted_policies, &turn.initiation))
            }
            None => None,
        };
        let policy = environments.command_policy();
        let mut listed = vec![EnvironmentListing {
            name: HERE.into(),
            here: true,
            reachable: Some(true),
            commands: false,
            current: current.is_none(),
        }];
        let hosts = environments
            .hosts()
            .iter()
            .map(|(name, environment)| (name.clone(), environment.clone()));
        for (name, environment) in hosts.chain(environments.connected_peers()) {
            let usable = match (policy, &initiator) {
                (Some(policy), Some(initiator)) => {
                    allows_host(policy.as_ref(), initiator.ask(), &name)
                }
                _ => false,
            };
            let is_current = current.as_deref() == Some(name.as_str());
            let shown = usable || is_current || matches!(initiator, Some(TurnInitiator::Owner));
            if !shown {
                continue;
            }
            let commands = environment.commands();
            listed.push(EnvironmentListing {
                here: false,
                reachable: commands.and_then(|commands| commands.reachable()),
                commands: commands.is_some() && usable,
                current: is_current,
                name,
            });
        }
        Ok(listed)
    }

    /// Of `names`, in their order, the ones a conversation owner's turns may
    /// run commands on: an environment away from here that takes commands,
    /// on a host the command policy lets the owner use. `agents.list` names
    /// these `commandHosts`, so a share dialog offers a Drive grant only the
    /// hosts it could use: a grant is never given a host its owner is not.
    /// Empty when this gateway grants no commands; `here` is never one.
    pub fn owner_command_hosts<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let environments = &self.inner.environment;
        let Some(policy) = environments.command_policy() else {
            return Vec::new();
        };
        names
            .into_iter()
            .filter(|name| {
                environments
                    .named(name)
                    .is_some_and(|environment| environment.commands().is_some())
                    && allows_host(policy.as_ref(), CommandInitiator::Owner, name)
            })
            .map(str::to_owned)
            .collect()
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
        let (parent, turn, actor, initiation) =
            running_turn(&snapshot, &call.call).ok_or(CommandCallError::NoTurn)?;
        // A Drive grant's turn runs a command only where its policy allows
        // `run` (row J28); where it then goes, and what it runs, the command
        // policy decides below for the same grant at the same revision.
        let checked = GatewayToolCall {
            conversation: id,
            turn: &turn,
            actor: &actor,
            initiation: &initiation,
            tool: "run",
        };
        if let Some(reason) = drive_refusal(&self.inner.drive, &live, checked).await {
            tracing::warn!(conversation_id = %id, %reason, "a Drive grant's tool call was refused");
            return Err(CommandCallError::NotAllowed(reason));
        }
        let lease = LeaseId::new(uuid::Uuid::new_v4().to_string())
            .expect("a UUID is a portable lease identity");
        // A name that is no environment is not recorded as one: nothing
        // names it.
        let named = EnvironmentRef::named(&call.environment).map_err(|_| {
            CommandCallError::Invalid(format!("{:?} names no environment", call.environment))
        })?;
        // A caller already gone is asked nothing.
        if *stop.borrow() {
            return Err(CommandCallError::Cancelled);
        }
        let terms = CommandTerms {
            environment: named.clone(),
            command: work.clone(),
            sandbox: call.sandbox,
        };
        let refused = Refused {
            lease: &lease,
            parent: &parent,
            terms: &terms,
            actor: &actor,
        };
        // Where: a host or paired environment this gateway knows, granted,
        // whose environment runs commands. Here runs none: its commands are
        // the agent's own shell's.
        let environment: Option<Arc<dyn Environment>> = match named {
            EnvironmentRef::Here => None,
            _ => self.inner.environment.named(&call.environment),
        };
        let Some(environment) = environment else {
            let refusal = if call.environment == HERE {
                CommandRefusal::CommandsUnavailable
            } else {
                CommandRefusal::EnvironmentNotGranted
            };
            return self.refuse(&live, &refused, refusal).await;
        };
        // A grant's policy no longer kept refuses the host, as the Drive
        // check above refuses its call: nothing stands in for it.
        let admitted = match TurnInitiator::of(&live.admitted_policies, &initiation) {
            Some(initiator) => admit_command(
                policy.as_ref(),
                initiator.ask(),
                &call.environment,
                work.program(),
            ),
            None => Err(CommandRefusal::EnvironmentNotGranted),
        };
        if let Err(refusal) = admitted {
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
        let granted = match commands.grant(&lease, &work).await {
            Ok(granted) => granted,
            Err(refusal) => return self.refuse(&live, &refused, refusal).await,
        };
        let Some(terms) = granted_terms(&terms, granted.granted) else {
            tracing::warn!(
                conversation_id = %id,
                lease = lease.as_str(),
                granted = ?granted.granted,
                "the environment granted a command other terms than asked; it does not run"
            );
            drop(granted.hold);
            let refusal = CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted);
            return self.refuse(&live, &refused, refusal).await;
        };
        let hold = granted.hold;
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
        let mut ran = hold.run(terms.command.timeout_ms(), stopped);
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
        // Recorded while its parent is still the conversation's latest lease.
        // Once a later agent lease replaced it, the stream's lease is that
        // later one and cannot take it: the parent's own end ended the
        // command, and this end is answered unrecorded and logged in full.
        let parent_for_end = parent.clone();
        let committed = manager
            .record_lease(move |current| {
                if current
                    .and_then(CurrentLease::held)
                    .is_some_and(|held| held.id() == &parent_for_end)
                {
                    (vec![ended], true)
                } else {
                    (Vec::new(), false)
                }
            })
            .await;
        // An end the records do not hold is not answered as if they did: an
        // unsaved one is saved once more, then the answer says it is not.
        let recorded = match &committed {
            Ok(commit) if !commit.decided => false,
            Ok(commit) if commit.saved.is_ok() => true,
            Ok(_) => manager.save_retained_lease_records().await.is_ok(),
            Err(_) => false,
        };
        if !recorded {
            tracing::error!(
                conversation_id = %id,
                lease = lease.as_str(),
                parent = parent.as_str(),
                exit = ?result.exit,
                cleanup = ?result.cleanup,
                stdout_bytes = result.stdout.len(),
                stderr_bytes = result.stderr.len(),
                dropped_bytes = result.dropped_bytes,
                replaced = matches!(&committed, Ok(commit) if !commit.decided),
                "a command's end could not be recorded"
            );
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
    pub(super) async fn live(&self, id: &ConversationId) -> Option<Arc<LiveConversation>> {
        let slot = self.inner.conversations.lock().await.get(id).cloned()?;
        slot.value.get()?.as_ref().ok().cloned()
    }
}

/// The terms to record for a command asked as `asked` whose environment
/// granted `granted`: the same command and sandbox, with the timeout it
/// granted, no longer than asked. `None` for any other answer, a lease
/// duration among them: a command is bounded by its timeout alone.
fn granted_terms(asked: &CommandTerms, granted: GrantedTerms) -> Option<CommandTerms> {
    if granted.lease_ms.is_some() {
        return None;
    }
    let timeout_ms = granted
        .timeout_ms
        .filter(|granted| *granted <= asked.command.timeout_ms())?;
    if granted.sandbox != asked.sandbox {
        return None;
    }
    let command = CommandWork::new(
        asked.command.argv().iter().map(|a| a.to_string()).collect(),
        asked.command.cwd().map(Into::into),
        timeout_ms,
    )
    .ok()?;
    Some(CommandTerms {
        command,
        ..asked.clone()
    })
}

/// What a refused command's record names.
struct Refused<'a> {
    lease: &'a LeaseId,
    parent: &'a LeaseId,
    terms: &'a CommandTerms,
    actor: &'a ActionContext,
}

/// The conversation's Live agent lease, the running turn, the person it is
/// for with `call` as their request, and who started it; `None` with no
/// turn running or no Live lease to run under.
pub(super) fn running_turn(
    snapshot: &SessionSnapshot,
    call: &str,
) -> Option<(LeaseId, ExecutionId, ActionContext, Initiation)> {
    let parent = snapshot
        .lease
        .as_ref()
        .and_then(CurrentLease::held)
        .filter(|lease| lease.phase() == LeasePhase::Live)?
        .id()
        .clone();
    let turn = running_invocation(snapshot)?;
    let actor = &turn.actor;
    let actor = ActionContext::new(actor.principal_id(), actor.surface_id(), call).ok()?;
    Some((
        parent,
        turn.request.execution_id.clone(),
        actor,
        turn.initiation.clone(),
    ))
}

/// Whether the command policy lets `initiator` run commands on `host` at all:
/// the one question `environments_list` and `agents.list` ask of it.
fn allows_host(
    policy: &dyn CommandPolicyEvaluator,
    initiator: CommandInitiator<'_>,
    host: &str,
) -> bool {
    matches!(
        policy.evaluate(initiator, CommandQuestion::Host { host }),
        Ok(ToolDecision::Allow { .. })
    )
}

/// The conversation's running turn, if one is.
fn running_invocation(snapshot: &SessionSnapshot) -> Option<&InvocationRecord> {
    snapshot.invocations.iter().rev().find(|record| {
        record.result.is_none()
            && record
                .scheduling
                .last()
                .is_some_and(|event| event.stage == InvocationStage::Running)
    })
}

/// Who a turn's commands run for, as the command policy is asked about
/// them: the owner, or the Drive grant that started it with its policy at
/// the revision the turn was admitted under.
enum TurnInitiator {
    Owner,
    Grant { grant: String, policy: ToolPolicy },
}

impl TurnInitiator {
    /// The initiator of a turn started as `initiation`, or `None` for a
    /// grant whose policy at that revision the conversation no longer keeps.
    fn of(admitted: &AdmittedPolicies, initiation: &Initiation) -> Option<Self> {
        match initiation.initiator() {
            Initiator::Owner => Some(Self::Owner),
            Initiator::Grant(grant) => {
                admitted
                    .policy(grant.grant(), grant.policy_revision())
                    .map(|policy| Self::Grant {
                        grant: grant.grant().to_owned(),
                        policy,
                    })
            }
        }
    }

    fn ask(&self) -> CommandInitiator<'_> {
        match self {
            Self::Owner => CommandInitiator::Owner,
            Self::Grant { grant, policy } => CommandInitiator::Grant { grant, policy },
        }
    }
}

/// One gateway tool call of a running turn, as [`drive_refusal`] checks it.
pub(super) struct GatewayToolCall<'a> {
    pub(super) conversation: &'a ConversationId,
    pub(super) turn: &'a ExecutionId,
    pub(super) actor: &'a ActionContext,
    pub(super) initiation: &'a Initiation,
    pub(super) tool: &'a str,
}

/// Why the gateway's tool `call.tool` may not run for a turn of `live`, or
/// `None` when it may: a Drive grant's turn is checked against its policy,
/// the owner's never is (row J28). A grant's decision, allowed or refused, is
/// committed to the Drive tool audit first; one that could not be is refused.
pub(super) async fn drive_refusal(
    drive: &OnceLock<DriveTools>,
    live: &LiveConversation,
    call: GatewayToolCall<'_>,
) -> Option<String> {
    let decided = decision(
        drive,
        &live.admitted_policies,
        call.initiation,
        ToolCall::new(ToolSource::Gateway, call.tool),
    )?;
    let Some(audit) = drive.get().map(|drive| drive.audit.clone()) else {
        return Some(decided.refusal.unwrap_or_else(|| unrecorded(call.tool)));
    };
    let record = DriveToolAuditRecord {
        conversation_id: call.conversation.clone(),
        turn: call.turn.as_str().to_owned(),
        call: call.actor.request_id().to_owned(),
        initiator_principal_id: call.actor.principal_id().to_owned(),
        initiator_surface_id: call.actor.surface_id().to_owned(),
        grant: decided.grant,
        revision: decided.revision,
        tool: call.tool.to_owned(),
        allowed: decided.refusal.is_none(),
        rules: decided.rules,
        reason: decided.refusal.clone(),
    };
    if let Err(error) = audit.record(record).await {
        tracing::error!(
            conversation_id = %call.conversation,
            call = call.actor.request_id(),
            tool = call.tool,
            ?error,
            "a Drive grant's tool decision could not be recorded, so the call is refused"
        );
        return Some(match decided.refusal {
            Some(reason) => format!("{reason}; its decision could not be recorded"),
            None => unrecorded(call.tool),
        });
    }
    decided.refusal
}

fn unrecorded(tool: &str) -> String {
    format!("the Drive grant's decision on {tool:?} could not be recorded, so it is refused")
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
