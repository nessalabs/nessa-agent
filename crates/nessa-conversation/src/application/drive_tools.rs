//! A Drive grant's turn calls only the tools its grant's policy allows
//! (slice J4 of #707). `docs/design/read-grants.md` holds the rules and the
//! state table its tests are written from.
//!
//! A turn names the grant it was started under and the revision admitted
//! (`Initiation`); the grant journal says what that revision's policy was
//! ([`ShareGrants::policy_at`]). Admission reads it once and keeps it for the
//! conversation in [`AdmittedPolicies`]. The turn proceeds only while
//! [`ShareGrants::hold_revision`] still finds that grant at that revision
//! (row J31), and it is saved as checked. The hold is that grant, not every
//! conversation. A native steer drops it after the save and compares the
//! revision again before the provider is asked. Every tool call of that turn is
//! then asked of [`ToolPolicyEvaluator`]:
//!
//! - a call the agent asks permission for, by [`DriveToolHook`] before a
//!   person is asked;
//! - the gateway's own `run`, `artifacts_publish` and `artifacts_fetch`, by
//!   the gateway before it runs them ([`decision`]). Each of these decisions,
//!   allowed or refused, is committed to [`DriveToolAudit`] first; a decision
//!   that could not be recorded refuses the call.
//!
//! ```text
//! send ─▶ admit: Claude or Codex, Ask mode, no native hooks, policy_at(rev)
//!           │                                     └─▶ AdmittedPolicies
//!           ▼
//!         hold_revision(rev) ─ this grant still at that revision? ─ else revoked
//!           ▼
//!         save the turn as checked
//!           │  native steer: drop the hold, compare the revision, then inject
//!           ▼
//!         drop the hold once the turn is saved (queued) or the compare returns
//!           ▼
//! turn ─▶ review hook ─────────▶ refusal(initiation, call) ─▶ Cedar
//! turn ─▶ run / publish / fetch ─▶ decision(initiation, call) ─▶ Cedar
//!                                        └─▶ DriveToolAudit, then answer
//! ```
//!
//! Only a call the agent asks about is a call this checks. In Ask mode
//! Claude reads files and Codex runs commands inside its read-only, offline
//! sandbox without asking: no policy takes those away, and a deny pattern
//! naming a read tool does not stop them. A directory grant is the exception.
//! Its turn is default-deny: a command is checked as a command, and a native
//! file or search tool is checked against the directory boundary. A tool
//! shape that is neither is refused. A call the harness makes without asking
//! is still recorded afterwards, not admitted here.
//!
//! The owner's turns are never checked here, and a missing or unreadable
//! policy refuses the call: nothing stands in for an answer the grant journal
//! could not give.

use super::share_grants::ShareGrants;
use super::tool_policy::{ToolDecision, ToolInitiator, ToolPolicyEvaluator};
use super::ConversationFuture;
use crate::domain::{SandboxKind, ToolCall, ToolPolicy, ToolSource};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::hooks::{
    HookError, InvocationContext, InvocationHook, InvocationObservationWindow, ProposedToolCall,
    ToolVerdict,
};
use nessa_sdk::domain::agent_execution::executions::{Initiation, Initiator};
use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

/// What checks a Drive grant's tools: the grant journal, the evaluator, and
/// where the gateway's own tool decisions are recorded. Composition binds it
/// once; a gateway that has not admits no Drive turn.
pub struct DriveTools {
    pub grants: Arc<dyn ShareGrants>,
    pub evaluator: Arc<dyn ToolPolicyEvaluator>,
    pub audit: Arc<dyn DriveToolAudit>,
}

/// Immutable evidence of one decision on a gateway tool call of a Drive
/// grant's turn, committed before the call is answered or run.
///
/// `allowed` with its `rules` is what the policy said; `reason` is why a
/// refused call was refused, which names an unavailable policy when there
/// were no rules to name. The sink assigns the observation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveToolAuditRecord {
    pub conversation_id: ConversationId,
    /// The turn the call was made in.
    pub turn: String,
    /// The tool call's identity.
    pub call: String,
    /// The person who started the turn under the grant, and from where.
    pub initiator_principal_id: String,
    pub initiator_surface_id: String,
    pub grant: String,
    pub revision: u64,
    pub tool: String,
    pub allowed: bool,
    pub rules: Vec<String>,
    pub reason: Option<String>,
}

/// Commits a gateway tool decision of a Drive turn. Records are idempotent by
/// conversation, call, tool and outcome: the same decision repeated is
/// acknowledged again, a contradicting one refused. An unavailable sink
/// refuses the call.
pub trait DriveToolAudit: Send + Sync {
    fn record(&self, record: DriveToolAuditRecord) -> ConversationFuture<'_, ()>;
}

/// What the policy of the Drive grant that started a turn said about one
/// call: whether it is allowed, the deciding policy ids, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveDecision {
    pub grant: String,
    pub revision: u64,
    pub rules: Vec<String>,
    /// `None` when the call is allowed.
    pub refusal: Option<String>,
}

/// The tool source of a conversation's agent, or `None` for an agent whose
/// calls are not all asked about: OpenCode runs the tools it allows without
/// asking, so a policy could judge only some of its calls.
pub fn checked_source(agent: AgentId) -> Option<ToolSource> {
    match agent {
        AgentId::Claude => Some(ToolSource::Claude),
        AgentId::Codex => Some(ToolSource::Codex),
        AgentId::Opencode => None,
    }
}

/// The most policies one conversation keeps for its admitted Drive turns.
/// A conversation holds at most eight Drive grants, and a policy is let go
/// of only once this many others were admitted since it last was; a turn
/// still needing it has its calls refused, never allowed.
pub const MAX_ADMITTED_POLICIES: usize = 64;

/// The tool policies of the Drive turns one conversation admitted, by grant
/// and revision. A revision's policy never changes, so a kept copy cannot
/// drift from the journal it was read from.
#[derive(Default)]
pub struct AdmittedPolicies {
    held: Mutex<VecDeque<(String, u64, ToolPolicy)>>,
}

impl AdmittedPolicies {
    /// Keep `policy` as `grant`'s at `revision`, as the one most recently
    /// admitted, letting go of the least recently admitted past
    /// [`MAX_ADMITTED_POLICIES`].
    pub fn admit(&self, grant: &str, revision: u64, policy: ToolPolicy) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.retain(|(kept, kept_revision, _)| !(kept == grant && *kept_revision == revision));
        if held.len() == MAX_ADMITTED_POLICIES {
            held.pop_front();
        }
        held.push_back((grant.to_owned(), revision, policy));
    }

    pub fn policy(&self, grant: &str, revision: u64) -> Option<ToolPolicy> {
        let held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.iter()
            .find(|(kept, kept_revision, _)| kept == grant && *kept_revision == revision)
            .map(|(_, _, policy)| policy.clone())
    }
}

/// What the policy of the Drive grant that started `initiation` says about
/// `call`, or `None` for the owner's turns, which are not checked. A grant's
/// call is refused when no Drive tools are bound, its policy was not kept,
/// the policy could not be evaluated, or the policy refuses it; the reason
/// names the grant, its revision, the tool and the deciding policy ids.
pub fn decision(
    drive: &OnceLock<DriveTools>,
    admitted: &AdmittedPolicies,
    initiation: &Initiation,
    call: ToolCall<'_>,
) -> Option<DriveDecision> {
    let Initiator::Grant(grant) = initiation.initiator() else {
        return None;
    };
    let (name, revision) = (grant.grant(), grant.policy_revision());
    let decided = |rules: Vec<String>, refusal: Option<String>| {
        Some(DriveDecision {
            grant: name.to_owned(),
            revision,
            rules,
            refusal,
        })
    };
    let unavailable = || {
        decided(
            Vec::new(),
            Some(format!(
                "the tool policy of Drive grant {name:?} at revision {revision} is unavailable, so {:?} is refused",
                call.name()
            )),
        )
    };
    let Some(drive) = drive.get() else {
        return unavailable();
    };
    let Some(policy) = admitted.policy(name, revision) else {
        return unavailable();
    };
    let initiator = ToolInitiator::Grant {
        grant: name,
        policy: &policy,
    };
    match drive.evaluator.evaluate(initiator, call) {
        Ok(ToolDecision::Allow { rules }) => decided(rules, None),
        Ok(ToolDecision::Refuse { rules }) => {
            let reason = format!(
                "Drive grant {name:?} at revision {revision} does not allow {:?} (rules: {})",
                call.name(),
                if rules.is_empty() {
                    "none allows it".to_owned()
                } else {
                    rules.join(", ")
                }
            );
            decided(rules, Some(reason))
        }
        Err(_) => unavailable(),
    }
}

/// Why `call` may not run for a turn started as `initiation`, or `None` when
/// it may: [`decision`]'s refusal.
pub fn refusal(
    drive: &OnceLock<DriveTools>,
    admitted: &AdmittedPolicies,
    initiation: &Initiation,
    call: ToolCall<'_>,
) -> Option<String> {
    decision(drive, admitted, initiation, call).and_then(|decided| decided.refusal)
}

/// Checks every call a conversation's agent asks permission for against the
/// tool policy of the Drive grant that started its turn, before a person is
/// asked. A refused call is answered as the hook and the turn goes on; the
/// refusal and its reason are saved with the review.
pub struct DriveToolHook {
    pub drive: Arc<OnceLock<DriveTools>>,
    pub admitted: Arc<AdmittedPolicies>,
    pub source: ToolSource,
    /// Directory grants, when composition bound them.
    pub directory: Arc<OnceLock<Arc<super::DirectoryAccess>>>,
}

impl InvocationHook for DriveToolHook {
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        if let Some(reason) = refusal(
            &self.drive,
            &self.admitted,
            call.initiation(),
            ToolCall::new(self.source, call.name()),
        ) {
            return Ok(ToolVerdict::refuse(reason, RefusalOutcome::Continue));
        }
        if let Some(reason) = directory_command(
            self,
            directory_subject(call.caller(), call.initiation()),
            call,
        ) {
            return Ok(ToolVerdict::refuse(reason, RefusalOutcome::Continue));
        }
        Ok(ToolVerdict::Allow)
    }

    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        _: &Result<
            nessa_sdk::domain::agent_execution::executions::ExecutionOutcome,
            nessa_sdk::application::agent_execution::agents::AgentError,
        >,
    ) -> Result<(), HookError> {
        let Some(directory) = self.directory.get() else {
            return Ok(());
        };
        let mut failure = None;
        for (_, name) in context.tool_audit.unasked() {
            if let Err(error) = directory
                .report_unasked(directory_subject(context.caller, context.initiation), name)
            {
                failure.get_or_insert(error);
            }
        }
        if context.tool_audit.incomplete() {
            failure.get_or_insert(super::directory_grant::DirectoryRefusal::Audit);
        }
        if let Some(error) = failure {
            return Err(HookError::Failed(error.to_string()));
        }
        Ok(())
    }
}

/// A paired grant is stored under its receiver. The turn's caller is the
/// principal, which is a different id.
pub(crate) fn directory_subject<'a>(caller: &'a str, initiation: &'a Initiation) -> &'a str {
    match initiation.initiator() {
        Initiator::Grant(grant) => grant.grant(),
        Initiator::Owner | Initiator::Agent(_) => caller,
    }
}

fn directory_command(
    hook: &DriveToolHook,
    subject: &str,
    call: &ProposedToolCall<'_>,
) -> Option<String> {
    let directory = hook.directory.get()?;
    review_directory_tool(directory, subject, call.name(), call.arguments_json())
}

/// Why a directory-granted tool review was refused, or `None` when it may run.
/// A subject with no directory grant is unchanged. A subject with one is
/// default-deny: a command is admitted as a command, a file or search tool is
/// admitted only for paths inside the directory, and every other shape is refused.
pub(crate) fn review_directory_tool(
    directory: &super::DirectoryAccess,
    subject: &str,
    name: &str,
    json: &str,
) -> Option<String> {
    match tool_shape(name, json) {
        ToolShape::Command(ParsedCommand::Skip) | ToolShape::Unknown => {
            deny_unknown(directory, subject)
        }
        ToolShape::Command(ParsedCommand::Unsafe(command)) => {
            if let Err(error) = directory.admit_shell(subject, &command, SandboxKind::None, &[]) {
                return Some(error.to_string());
            }
            match directory.command_constrained(subject) {
                Ok(false) => None,
                Ok(true) => Some("the command is not a single program".to_owned()),
                Err(error) => Some(error.to_string()),
            }
        }
        ToolShape::Command(ParsedCommand::Run { program, arguments }) => {
            match directory.admit_execution(subject, &program, &arguments, SandboxKind::None, &[]) {
                Ok(()) => None,
                Err(error) => Some(error.to_string()),
            }
        }
        ToolShape::Files { capability, paths } => {
            if paths.is_empty() {
                return deny_unbounded(directory, subject);
            }
            for path in paths {
                if let Err(error) = directory.admit_path(subject, &path, capability) {
                    return Some(error.to_string());
                }
            }
            None
        }
        ToolShape::Unbounded => deny_unbounded(directory, subject),
    }
}

fn deny_unknown(directory: &super::DirectoryAccess, subject: &str) -> Option<String> {
    match directory.subject_has_directory(subject) {
        Ok(false) => None,
        Ok(true) => Some("the tool is not admitted for this directory".to_owned()),
        Err(error) => Some(error.to_string()),
    }
}

fn deny_unbounded(directory: &super::DirectoryAccess, subject: &str) -> Option<String> {
    match directory.subject_has_directory(subject) {
        Ok(false) => None,
        Ok(true) => Some("the tool does not name a path inside the directory".to_owned()),
        Err(error) => Some(error.to_string()),
    }
}

/// A review a directory grant can check, or a shape it cannot.
enum ToolShape {
    Command(ParsedCommand),
    /// Each path is checked for `capability` (`read` or `write`).
    Files {
        capability: &'static str,
        paths: Vec<String>,
    },
    /// A file or search tool that names no path, so the boundary cannot be checked.
    Unbounded,
    /// Not a command and not a file tool.
    Unknown,
}

fn tool_shape(name: &str, json: &str) -> ToolShape {
    let Some(value) = argument_object(json) else {
        return ToolShape::Unknown;
    };
    if name != "apply_patch" && (value.get("argv").is_some() || value.get("command").is_some()) {
        return ToolShape::Command(command_from_value(&value, json));
    }
    match name {
        "Read" => files("read", file_path(&value)),
        "Write" | "Edit" => files("write", file_path(&value)),
        "NotebookEdit" => files("write", notebook_path(&value)),
        "MultiEdit" => files("write", edit_paths(&value)),
        "Glob" | "Grep" | "LS" => files("read", search_path(&value)),
        "apply_patch" => files("write", patch_paths(&value)),
        _ => {
            if let Some(path) = text_at(&value, "file_path") {
                ToolShape::Files {
                    capability: "write",
                    paths: vec![path],
                }
            } else if let Some(paths) = string_list(value.get("paths")) {
                files("write", paths)
            } else {
                ToolShape::Unknown
            }
        }
    }
}

fn files(capability: &'static str, paths: Vec<String>) -> ToolShape {
    if paths.is_empty() {
        ToolShape::Unbounded
    } else {
        ToolShape::Files { capability, paths }
    }
}

fn argument_object(json: &str) -> Option<serde_json::Value> {
    let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
    if !value.is_object() {
        return None;
    }
    if value.get("arguments").is_some()
        && value.get("command").is_none()
        && value.get("argv").is_none()
        && value.get("file_path").is_none()
    {
        return value
            .get("arguments")
            .cloned()
            .filter(|item| item.is_object());
    }
    Some(value)
}

fn command_from_value(value: &serde_json::Value, json: &str) -> ParsedCommand {
    if let Some(argv) = value.get("argv").and_then(|item| item.as_array()) {
        let mut parts = Vec::new();
        for item in argv {
            let Some(text) = item.as_str() else {
                return ParsedCommand::Unsafe(json.to_owned());
            };
            parts.push(text.to_owned());
        }
        let Some(program) = parts.first().cloned() else {
            return ParsedCommand::Unsafe(json.to_owned());
        };
        return ParsedCommand::Run {
            program,
            arguments: parts.into_iter().skip(1).collect(),
        };
    }
    let Some(command) = value.get("command").and_then(|item| item.as_str()) else {
        return ParsedCommand::Skip;
    };
    command_arguments(&serde_json::json!({ "command": command }).to_string())
}

fn file_path(value: &serde_json::Value) -> Vec<String> {
    text_at(value, "file_path").into_iter().collect()
}

fn notebook_path(value: &serde_json::Value) -> Vec<String> {
    text_at(value, "notebook_path")
        .or_else(|| text_at(value, "file_path"))
        .into_iter()
        .collect()
}

fn search_path(value: &serde_json::Value) -> Vec<String> {
    text_at(value, "path")
        .or_else(|| text_at(value, "file_path"))
        .into_iter()
        .collect()
}

fn edit_paths(value: &serde_json::Value) -> Vec<String> {
    let mut paths = file_path(value);
    if let Some(edits) = value.get("edits").and_then(|item| item.as_array()) {
        for edit in edits {
            if let Some(path) = text_at(edit, "file_path") {
                paths.push(path);
            }
        }
    }
    paths
}

fn patch_paths(value: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["path", "file_path"] {
        if let Some(path) = text_at(value, key) {
            paths.push(path);
        }
    }
    if let Some(files) = value.get("files").and_then(|item| item.as_array()) {
        for file in files {
            if let Some(path) = file.as_str() {
                paths.push(path.to_owned());
            } else if let Some(path) = text_at(file, "path").or_else(|| text_at(file, "file_path"))
            {
                paths.push(path);
            }
        }
    }
    for key in ["input", "patch", "command"] {
        if let Some(text) = text_at(value, key) {
            paths.extend(paths_in_patch(&text));
        }
    }
    paths
}

fn paths_in_patch(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        for prefix in [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                let path = path.trim();
                if !path.is_empty() {
                    paths.push(path.to_owned());
                }
            }
        }
    }
    paths
}

fn text_at(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn string_list(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let items = value?.as_array()?;
    let mut paths = Vec::new();
    for item in items {
        let text = item.as_str().filter(|text| !text.is_empty())?;
        paths.push(text.to_owned());
    }
    Some(paths)
}

/// What a permission review's arguments are, as a command.
#[derive(Debug)]
enum ParsedCommand {
    /// The review is not a command.
    Skip,
    /// A shell string whose programs cannot all be named. The string is the
    /// original input, kept for the refusal audit.
    Unsafe(String),
    /// One program and its literal arguments.
    Run {
        program: String,
        arguments: Vec<String>,
    },
}

/// A command the harness is asking about: an `argv` array of literals, or a
/// `command` string that is one program and its arguments. A string the shell
/// would split into more than those tokens is [`ParsedCommand::Unsafe`].
fn command_arguments(json: &str) -> ParsedCommand {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return ParsedCommand::Skip;
    };
    let value = value
        .get("arguments")
        .filter(|item| item.get("command").is_some() || item.get("argv").is_some())
        .unwrap_or(&value);
    if let Some(argv) = value.get("argv").and_then(|item| item.as_array()) {
        let mut parts = Vec::new();
        for item in argv {
            let Some(text) = item.as_str() else {
                return ParsedCommand::Unsafe(json.to_owned());
            };
            parts.push(text.to_owned());
        }
        let Some(program) = parts.first().cloned() else {
            return ParsedCommand::Unsafe(json.to_owned());
        };
        return ParsedCommand::Run {
            program,
            arguments: parts.into_iter().skip(1).collect(),
        };
    }
    let Some(command) = value.get("command").and_then(|item| item.as_str()) else {
        return ParsedCommand::Skip;
    };
    if command
        .chars()
        .any(|character| character.is_whitespace() && character != ' ')
    {
        return ParsedCommand::Unsafe(command.to_owned());
    }
    let mut parts = Vec::new();
    for token in command.split_whitespace() {
        if !literal_token(token) {
            return ParsedCommand::Unsafe(command.to_owned());
        }
        parts.push(token.to_owned());
    }
    let Some(program) = parts.first().cloned() else {
        return ParsedCommand::Unsafe(command.to_owned());
    };
    ParsedCommand::Run {
        program,
        arguments: parts.into_iter().skip(1).collect(),
    }
}

/// A token a shell runs as itself: no metacharacter, quote, or expansion.
fn literal_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|character| {
            matches!(
                character,
                'a'..='z'
                    | 'A'..='Z'
                    | '0'..='9'
                    | '/'
                    | '.'
                    | '_'
                    | '-'
                    | '+'
                    | '='
                    | ':'
                    | '@'
                    | ','
            )
        })
}

#[cfg(test)]
mod command_parse {
    use super::{command_arguments, ParsedCommand};

    #[test]
    fn a_shell_string_that_runs_another_program_is_refused() {
        assert!(matches!(
            command_arguments(r#"{"command":"ls .; rm victim"}"#),
            ParsedCommand::Unsafe(_)
        ));
        assert!(matches!(
            command_arguments(r#"{"command":"ls | rm"}"#),
            ParsedCommand::Unsafe(_)
        ));
        assert!(matches!(
            command_arguments(r#"{"command":"ls $(rm victim)"}"#),
            ParsedCommand::Unsafe(_)
        ));
        assert!(matches!(
            command_arguments("{\"command\":\"ls .\\nrm victim\"}"),
            ParsedCommand::Unsafe(_)
        ));
        match command_arguments(
            r#"{"server":"nessa","tool":"shell","arguments":{"command":"cat .nessa-binding"}}"#,
        ) {
            ParsedCommand::Run { program, arguments } => {
                assert_eq!(program, "cat");
                assert_eq!(arguments, [".nessa-binding"]);
            }
            other => panic!("{other:?}"),
        }
        match command_arguments(r#"{"command":"cat \".nessa-binding\""}"#) {
            ParsedCommand::Unsafe(command) => assert_eq!(command, "cat \".nessa-binding\""),
            other => panic!("{other:?}"),
        }
        match command_arguments(r#"{"command":"ls -l"}"#) {
            ParsedCommand::Run { program, arguments } => {
                assert_eq!(program, "ls");
                assert_eq!(arguments, ["-l"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_paired_grant_is_checked_as_its_receiver() {
        use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
        let grant = GrantInitiator::new("dev-1", 4).unwrap();
        assert_eq!(
            super::directory_subject("principal-x", &Initiation::checked_grant(grant)),
            "dev-1"
        );
        assert_eq!(
            super::directory_subject("owner-1", &Initiation::owner()),
            "owner-1"
        );
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/drive_tools.rs"]
mod tests;
