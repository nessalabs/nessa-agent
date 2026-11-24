//! Sharing one directory: a binding marker, a path walk, and command rules.
//!
//! Cedar decides every read, write, exec, and command. This module orders
//! the facts, the decision, and the audit. It does not open files itself.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::domain::{
    encoded_grant, join_argv, ArgvPattern, CommandMatcher, CommandRules, DirectoryCapabilities,
    DirectoryShapeError, PathLeave, RuleEffect, SandboxKind, WalkFacts, BINDING_LEN,
};
use crate::timer::clock::wall_now;

/// Why a directory operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectoryRefusal {
    /// The grant's shape or size was refused before any file was touched.
    Shape(DirectoryShapeError),
    /// The filesystem could not complete the walk.
    Io(std::io::ErrorKind),
    /// The grant store could not record the change.
    Store,
    /// Cedar could not decide.
    PolicyUnavailable,
    /// The marker was present and was not exactly 32 bytes. It was not rewritten.
    MarkerSize,
    /// Those marker bytes already belong to a directory at another path.
    BindingCopied,
    /// The named path left the granted directory.
    Outside {
        leave: PathLeave,
        audit_recorded: bool,
    },
    /// The named path is the marker file.
    MarkerName { audit_recorded: bool },
    /// The marker was missing or different. Grants for the resource are revoked.
    MarkerMismatch { cause: String, audit_recorded: bool },
    /// Exec was refused. `rules` are the Cedar policy ids.
    Exec {
        rules: Vec<String>,
        audit_recorded: bool,
    },
    /// A command matcher refused the program.
    Program {
        matcher: String,
        rules: Vec<String>,
        audit_recorded: bool,
    },
    /// Command rules are a constraint, and the harness hooks would answer first.
    HooksAnswerFirst,
    /// The grant does not include this capability.
    Capability {
        rules: Vec<String>,
        audit_recorded: bool,
    },
    /// A marker this attempt created is still waiting to be removed.
    CleanupPending,
    /// No grant exists for this subject.
    NotFound,
    /// The audit sink did not accept the record. The operation is not done.
    Audit,
}

impl std::fmt::Display for DirectoryRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(DirectoryShapeError::Frame) => {
                write!(formatter, "the directory grant does not fit one frame")
            }
            Self::Shape(_) => write!(formatter, "the directory grant shape was refused"),
            Self::Io(_) => write!(formatter, "the directory could not be opened"),
            Self::Store => write!(formatter, "the directory grant could not be stored"),
            Self::PolicyUnavailable => write!(formatter, "directory policy could not decide"),
            Self::MarkerSize => write!(formatter, "the directory marker is the wrong size"),
            Self::BindingCopied => write!(formatter, "the directory marker is a copy"),
            Self::Outside {
                leave,
                audit_recorded,
            } => write!(
                formatter,
                "the path left the directory ({leave:?}){}",
                audit_note(*audit_recorded)
            ),
            Self::MarkerName { audit_recorded } => write!(
                formatter,
                "the directory marker cannot be used{}",
                audit_note(*audit_recorded)
            ),
            Self::MarkerMismatch { audit_recorded, .. } => write!(
                formatter,
                "the directory marker does not match{}",
                audit_note(*audit_recorded)
            ),
            Self::Exec { audit_recorded, .. } => {
                write!(
                    formatter,
                    "the command was not admitted{}",
                    audit_note(*audit_recorded)
                )
            }
            Self::Program {
                matcher,
                audit_recorded,
                ..
            } => write!(
                formatter,
                "command rule {matcher} refused the program{}",
                audit_note(*audit_recorded)
            ),
            Self::HooksAnswerFirst => {
                write!(
                    formatter,
                    "harness hooks would answer before the command rules"
                )
            }
            Self::Capability { audit_recorded, .. } => write!(
                formatter,
                "the directory grant does not allow this{}",
                audit_note(*audit_recorded)
            ),
            Self::CleanupPending => {
                write!(formatter, "the directory marker cleanup is still pending")
            }
            Self::NotFound => write!(formatter, "no directory grant"),
            Self::Audit => write!(formatter, "the directory audit was not recorded"),
        }
    }
}

enum RefusalKind {
    Marker,
    Path,
    Capability,
}

fn audit_note(recorded: bool) -> &'static str {
    if recorded {
        ""
    } else {
        "; the audit was not recorded"
    }
}

/// What the marker file held, stopping at 33 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerBytes {
    /// The file was not there.
    Absent,
    /// Exactly 32 bytes.
    Exact([u8; BINDING_LEN]),
    /// Any other size. The file is not rewritten.
    Other,
    /// The name is not a regular file. Grants that named it are revoked.
    Unusable,
}

/// A subject of a directory grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectorySubject {
    /// Principal id, when the subject is a principal.
    pub principal_id: Option<String>,
    /// Receiver id, when the subject is a receiver.
    pub receiver_id: Option<String>,
}

/// A request to share one directory.
#[derive(Clone, Debug)]
pub struct ShareDirectory {
    /// The environment the grantor enforces.
    pub environment: String,
    /// The directory the owner named.
    pub path: String,
    /// Who receives the grant.
    pub subject: DirectorySubject,
    /// Read, write, and exec, any non-empty subset.
    pub capabilities: DirectoryCapabilities,
    /// Ordered command matchers. Empty is no extra command constraint.
    pub rules: CommandRules,
    /// Who asked.
    pub initiator: String,
}

/// The grant that was stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedDirectory {
    /// The resource id. A second subject of the same directory reuses it.
    pub resource_id: String,
    /// The canonical directory.
    pub canonical_path: String,
    /// The capability spelling.
    pub capabilities: String,
}

/// One stored grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDirectoryGrant {
    /// The resource id.
    pub resource_id: String,
    /// Principal, when the subject is a principal.
    pub principal_id: Option<String>,
    /// Receiver, when the subject is a receiver.
    pub receiver_id: Option<String>,
    /// Capability spelling.
    pub capabilities: String,
    /// Command rules, when the grant has any.
    pub rules: CommandRules,
}

/// One stored directory resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDirectoryResource {
    /// Resource id.
    pub resource_id: String,
    /// Environment.
    pub environment: String,
    /// Canonical path.
    pub canonical_path: String,
    /// The 32 marker bytes.
    pub binding: [u8; BINDING_LEN],
}

/// One opened directory. Every check and every read or write uses this handle.
pub trait DirectorySession: Send {
    /// The marker, read from this directory.
    fn marker(&self) -> Result<MarkerBytes, DirectoryRefusal>;
    /// Walk `relative` from this directory.
    fn walk(&self, relative: &str) -> Result<WalkFacts, DirectoryRefusal>;
    /// Read a file from this directory.
    fn read(&self, relative: &str) -> Result<Vec<u8>, DirectoryRefusal>;
    /// Write a file in this directory, without truncating the marker.
    fn write(&self, relative: &str, bytes: &[u8]) -> Result<(), DirectoryRefusal>;
    /// Where each sandbox root sits relative to this directory.
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal>;
}

/// Facts the walk reports.
pub trait DirectoryFiles: Send + Sync {
    /// The canonical directory, opened no-follow.
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal>;
    /// The marker, stopping at 33 bytes.
    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal>;
    /// Create the marker exclusively with these bytes.
    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal>;
    /// Remove the marker. Absent is success.
    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal>;
    /// The marker this attempt created is no longer at `canonical`. A different
    /// file now at that name is left in place. `Ok(false)` means the matching
    /// marker is still there.
    fn release_created_marker(
        &self,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
    ) -> Result<bool, DirectoryRefusal> {
        let _ = binding;
        self.unlink_marker(canonical).map(|()| true)
    }
    /// Walk `relative` from the granted directory.
    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal>;
    /// Read a file the walk already placed inside.
    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal>;
    /// Write a file the walk already placed inside, without truncating the marker.
    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal>;
    /// Whether a process in this sandbox can open the marker.
    fn hides_marker(&self, profile: SandboxKind) -> bool;
    /// Where each sandbox root sits relative to the granted directory.
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal>;
    /// Open the directory once. The handle stays the directory that was opened.
    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        let _ = canonical;
        Err(DirectoryRefusal::Io(std::io::ErrorKind::Unsupported))
    }
}

/// The grant rows for one directory.
pub trait DirectoryStore: Send + Sync {
    /// Insert a new resource and the audit that transition owes, in one commit.
    /// The binding must be unique.
    fn insert_resource(
        &self,
        resource: &StoredDirectoryResource,
        transition: &DirectoryAuditRecord,
    ) -> Result<(), DirectoryRefusal>;
    /// The resource at this environment and path.
    fn resource_at(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal>;
    /// The resource with this id.
    fn resource(
        &self,
        resource_id: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal>;
    /// The resource that already holds these binding bytes.
    fn resource_by_binding(
        &self,
        binding: &[u8; BINDING_LEN],
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal>;
    /// Insert one grant.
    fn insert_grant(
        &self,
        grant: &StoredDirectoryGrant,
        initiator: &str,
    ) -> Result<(), DirectoryRefusal>;
    /// Grants whose subject is this principal, receiver, or resource id.
    fn grants_for_subject(
        &self,
        subject: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal>;
    /// Grants for one resource.
    fn grants_for_resource(
        &self,
        resource_id: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal>;
    /// Revoke every current grant for the resource. An earlier revocation does not
    /// leave a later share in place.
    fn revoke_mismatch(
        &self,
        resource_id: &str,
        cause: &str,
        initiator: &str,
    ) -> Result<RevokeMismatch, DirectoryRefusal>;
    /// Mark this revocation recorded. A different epoch is left pending.
    fn mark_open_audit(&self, resource_id: &str, epoch: i64) -> Result<(), DirectoryRefusal>;
    /// An open audit that has not been recorded.
    fn pending_open_audit(
        &self,
        resource_id: &str,
    ) -> Result<Option<PendingOpenAudit>, DirectoryRefusal>;
    /// Resources whose mismatch audit has not been recorded.
    fn open_audits_pending(&self) -> Result<Vec<String>, DirectoryRefusal> {
        Ok(Vec::new())
    }
    /// Resources that still have an audit the sink refused.
    fn pending_resources(&self) -> Result<Vec<String>, DirectoryRefusal> {
        Ok(Vec::new())
    }
    /// Remember a marker this attempt created whose resource row did not commit.
    fn record_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
        initiator: &str,
    ) -> Result<(), DirectoryRefusal>;
    /// The cleanup row, if one is still pending.
    fn cleanup(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<PendingCleanup>, DirectoryRefusal>;
    /// Every cleanup that still owes an unlink or an audit. Each pair is the
    /// environment and the canonical path.
    fn pending_cleanups(&self) -> Result<Vec<(String, String)>, DirectoryRefusal> {
        Ok(Vec::new())
    }
    /// Record that the marker was unlinked, or that the cleanup audit committed.
    fn update_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        unlinked: bool,
        audit_done: bool,
    ) -> Result<(), DirectoryRefusal>;
    /// Remove the cleanup row once both the unlink and the audit have committed.
    fn clear_cleanup(&self, environment: &str, canonical: &str) -> Result<(), DirectoryRefusal>;
    /// Remember an audit the sink refused.
    fn save_pending(&self, record: &DirectoryAuditRecord) -> Result<(), DirectoryRefusal> {
        let _ = record;
        Err(DirectoryRefusal::Store)
    }
    /// Audits still waiting for this resource.
    fn pending_for(&self, resource_id: &str) -> Result<Vec<PendingAudit>, DirectoryRefusal> {
        let _ = resource_id;
        Ok(Vec::new())
    }
    /// Forget one delivered pending audit.
    fn drop_pending(&self, sequence: i64) -> Result<(), DirectoryRefusal> {
        let _ = sequence;
        Ok(())
    }
}

/// An audit record the sink has not accepted yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAudit {
    /// Row id.
    pub sequence: i64,
    /// The record to deliver.
    pub record: DirectoryAuditRecord,
}

/// The first mismatch, and whether this call revoked the grants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevokeMismatch {
    /// Cause stored on the first open.
    pub cause: String,
    /// Initiator stored on the first open.
    pub initiator: String,
    /// Whether this call wrote the revoke.
    pub removed_now: bool,
    /// The revocation this call observed. An acknowledgement names this epoch.
    pub epoch: i64,
}

/// An open audit waiting for the sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOpenAudit {
    /// The original cause.
    pub cause: String,
    /// The original initiator.
    pub initiator: String,
    /// The revocation this row is.
    pub epoch: i64,
}

/// A marker waiting to be unlinked and audited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCleanup {
    /// Marker bytes this attempt created.
    pub binding: [u8; BINDING_LEN],
    /// The unlink has been recorded.
    pub unlinked: bool,
    /// The cleanup audit has been recorded.
    pub audit_done: bool,
    /// Who asked for the share.
    pub initiator: String,
}

/// Cedar's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyAnswer {
    /// Permitted.
    Allow,
    /// Refused, naming the policy ids.
    Deny { rules: Vec<String> },
}

/// A read or write question.
#[derive(Clone, Debug)]
pub struct UseQuestion<'a> {
    /// Capability names.
    pub capabilities: Vec<String>,
    /// Resource id.
    pub resource: String,
    /// `directory.read` or `directory.write`.
    pub action: String,
    /// The walk.
    pub leave: PathLeave,
    /// Whether the opened entry is the marker.
    pub marker: bool,
    /// Unused lifetime so a caller can borrow.
    pub _borrow: std::marker::PhantomData<&'a ()>,
}

/// An exec admission question.
#[derive(Clone, Debug)]
pub struct ExecQuestion {
    /// Capability names.
    pub capabilities: Vec<String>,
    /// Resource id.
    pub resource: String,
    /// Sandbox profile.
    pub profile: SandboxKind,
    /// The process cannot open the marker.
    pub hides_marker: bool,
    /// Where the sandbox roots sit.
    pub root_leave: PathLeave,
}

/// A command-rule question.
#[derive(Clone, Debug)]
pub struct DirectoryCommandQuestion<'a> {
    /// Capability names.
    pub capabilities: Vec<String>,
    /// The matchers.
    pub rules: &'a CommandRules,
    /// Program path or file name.
    pub program: &'a str,
    /// Arguments joined by U+001F.
    pub argv: String,
}

/// Cedar for directory grants.
pub trait DirectoryPolicyEvaluator: Send + Sync {
    /// Read or write.
    fn use_directory(&self, question: &UseQuestion<'_>) -> Result<PolicyAnswer, PolicyUnavailable>;
    /// Exec admission.
    fn exec(&self, question: &ExecQuestion) -> Result<PolicyAnswer, PolicyUnavailable>;
    /// One command against the grant's matchers.
    fn command(
        &self,
        question: &DirectoryCommandQuestion<'_>,
    ) -> Result<PolicyAnswer, PolicyUnavailable>;
}

/// The audit sink could not record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyUnavailable;

/// The audit sink could not record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditUnavailable;

/// One directory audit line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DirectoryAuditRecord {
    /// What happened.
    pub cause: String,
    /// Resource id, when one exists.
    pub resource_id: Option<String>,
    /// Environment.
    pub environment: String,
    /// Canonical path.
    pub canonical_path: String,
    /// Who.
    pub initiator: String,
    /// Capability before, when a grant changed.
    pub before: Option<String>,
    /// Capability after.
    pub after: Option<String>,
    /// Sandbox profile, for an admitted exec.
    pub profile: Option<String>,
    /// When.
    pub at_ms: u64,
}

/// Append-only audit.
pub trait DirectoryAudit: Send + Sync {
    /// Record one line. Failure means the sink is down.
    fn record(&self, record: &DirectoryAuditRecord) -> Result<(), AuditUnavailable>;
}

/// 32 fresh binding bytes.
pub trait BindingSource: Send + Sync {
    /// The next binding.
    fn mint(&self) -> [u8; BINDING_LEN];
}

/// Random binding bytes.
pub struct RandomBinding;

impl BindingSource for RandomBinding {
    fn mint(&self) -> [u8; BINDING_LEN] {
        let mut bytes = [0; BINDING_LEN];
        getrandom::fill(&mut bytes).expect("binding bytes");
        bytes
    }
}

/// One exec admission.
pub struct ExecAsk<'a> {
    /// Who.
    pub subject: &'a str,
    /// Program file name.
    pub program: &'a str,
    /// Arguments after the program.
    pub argv: &'a [String],
    /// A shell string that is not one literal program. The refusal audit keeps
    /// this text rather than an empty argv.
    pub shell: Option<&'a str>,
    /// Sandbox profile.
    pub profile: SandboxKind,
    /// Sandbox roots.
    pub roots: &'a [String],
    /// The harness hooks would answer before Nessa.
    pub hooks_answer_first: bool,
}

/// The directory grant service.
pub struct DirectoryAccess {
    files: Arc<dyn DirectoryFiles>,
    store: Arc<dyn DirectoryStore>,
    policy: Arc<dyn DirectoryPolicyEvaluator>,
    audit: Arc<dyn DirectoryAudit>,
    binding: Arc<dyn BindingSource>,
    leases: std::sync::Mutex<std::collections::HashMap<String, Arc<Lease>>>,
}

struct Lease {
    held: std::sync::Mutex<bool>,
    free: std::sync::Condvar,
}

impl DirectoryAccess {
    /// The ports this service uses.
    pub fn new(
        files: Arc<dyn DirectoryFiles>,
        store: Arc<dyn DirectoryStore>,
        policy: Arc<dyn DirectoryPolicyEvaluator>,
        audit: Arc<dyn DirectoryAudit>,
        binding: Arc<dyn BindingSource>,
    ) -> Self {
        Self {
            files,
            store,
            policy,
            audit,
            binding,
            leases: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Share one directory with one subject.
    pub fn share(&self, request: ShareDirectory) -> Result<SharedDirectory, DirectoryRefusal> {
        self.reconcile()?;
        let canonical = self.files.canonicalize(&request.path)?;
        encoded_grant(
            &request.environment,
            &canonical,
            request.capabilities,
            &request.rules,
        )
        .map_err(DirectoryRefusal::Shape)?;
        let _lease = self.lease(&request.environment, &canonical);
        self.finish_cleanup_held(&request.environment, &canonical)?;
        match self.files.read_marker(&canonical)? {
            MarkerBytes::Other => Err(DirectoryRefusal::MarkerSize),
            MarkerBytes::Exact(bytes) => self.share_existing(request, canonical, bytes),
            MarkerBytes::Absent => self.share_new(request, canonical),
            MarkerBytes::Unusable => {
                if let Some(resource) = self.store.resource_at(&request.environment, &canonical)? {
                    return self.mismatch(&resource, "marker-type", &request.initiator);
                }
                Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput))
            }
        }
    }

    /// Read a file inside a subject's directory.
    pub fn read(&self, subject: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        let (grant, resource, session) = self.prepare(subject, relative, "read")?;
        let bytes = session.read(relative)?;
        let facts = session.walk(relative)?;
        self.refuse_walk(&grant, &resource, facts, subject, relative, "read")?;
        Ok(bytes)
    }

    /// Write a file inside a subject's directory. A missing final name, under a
    /// parent that is inside the directory, is created.
    pub fn write(
        &self,
        subject: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        let (_grant, _resource, session) = self.prepare(subject, relative, "write")?;
        session.write(relative, bytes)
    }

    /// Admit exec. This does not spawn a process.
    pub fn exec(&self, ask: ExecAsk<'_>) -> Result<(), DirectoryRefusal> {
        let owed = self.reconcile().err();
        match self.decide_exec(ask) {
            Ok(()) => owed.map_or(Ok(()), Err),
            Err(error) => Err(error),
        }
    }

    fn decide_exec(&self, ask: ExecAsk<'_>) -> Result<(), DirectoryRefusal> {
        let grants = self.store.grants_for_subject(ask.subject)?;
        let Some(grant) = grants.first() else {
            return Err(DirectoryRefusal::NotFound);
        };
        if grant.rules.constrains_commands() && ask.hooks_answer_first {
            return Err(DirectoryRefusal::HooksAnswerFirst);
        }
        let resources = self.resource_of(grant)?;
        let session = self.open_root(&resources, ask.subject)?;
        self.open_marker_on(&*session, &resources, ask.subject)?;
        self.flush_pending(&grant.resource_id)?;
        let root_leave = session.root_leave(&resources.canonical_path, ask.roots)?;
        let question = ExecQuestion {
            capabilities: capability_names(&grant.capabilities),
            resource: grant.resource_id.clone(),
            profile: ask.profile,
            hides_marker: self.files.hides_marker(ask.profile),
            root_leave,
        };
        match self
            .policy
            .exec(&question)
            .map_err(|_| DirectoryRefusal::PolicyUnavailable)?
        {
            PolicyAnswer::Allow => self.audit_required(DirectoryAuditRecord {
                cause: "exec".to_owned(),
                resource_id: Some(grant.resource_id.clone()),
                environment: resources.environment,
                canonical_path: resources.canonical_path,
                initiator: ask.subject.to_owned(),
                before: None,
                after: None,
                profile: Some(ask.profile.as_str().to_owned()),
                at_ms: now_ms(),
            }),
            PolicyAnswer::Deny { rules } => {
                let recorded = self.deliver_or_keep(&DirectoryAuditRecord {
                    cause: "exec-refused".to_owned(),
                    resource_id: Some(grant.resource_id.clone()),
                    environment: resources.environment,
                    canonical_path: resources.canonical_path,
                    initiator: ask.subject.to_owned(),
                    before: (!rules.is_empty()).then(|| rules.join(",")),
                    after: Some(match ask.shell {
                        Some(command) => command.to_owned(),
                        None => attempted_command(ask.program, ask.argv),
                    }),
                    profile: Some(ask.profile.as_str().to_owned()),
                    at_ms: now_ms(),
                });
                Err(DirectoryRefusal::Exec {
                    rules,
                    audit_recorded: recorded,
                })
            }
        }
    }

    /// Apply every directory grant this subject holds. A grant with no command
    /// list allows. A deny on any grant refuses, including when another grant
    /// has no list.
    pub fn command_at(
        &self,
        subject: &str,
        program: &str,
        argv: &[String],
    ) -> Result<(), DirectoryRefusal> {
        self.reconcile()?;
        self.enforce_commands(subject, program, argv)
    }

    fn enforce_commands(
        &self,
        subject: &str,
        program: &str,
        argv: &[String],
    ) -> Result<(), DirectoryRefusal> {
        for grant in self.store.grants_for_subject(subject)? {
            self.command_on(subject, &grant, program, argv)?;
        }
        Ok(())
    }

    /// Command rules, then exec admission, for a command this subject will run.
    /// No directory grant is not an exec refusal.
    pub fn admit_execution(
        &self,
        subject: &str,
        program: &str,
        argv: &[String],
        profile: SandboxKind,
        roots: &[String],
    ) -> Result<(), DirectoryRefusal> {
        if self.store.grants_for_subject(subject)?.is_empty() {
            return self.reconcile();
        }
        self.exec(ExecAsk {
            subject,
            program,
            argv,
            shell: None,
            profile,
            roots,
            hooks_answer_first: false,
        })?;
        self.enforce_commands(subject, program, argv)
    }

    /// Exec admission for a shell string that is not one literal program.
    /// A subject with no directory grant is unchanged. A subject with a grant
    /// is admitted only when exec allows that sandbox.
    pub fn admit_shell(
        &self,
        subject: &str,
        command: &str,
        profile: SandboxKind,
        roots: &[String],
    ) -> Result<(), DirectoryRefusal> {
        if self.store.grants_for_subject(subject)?.is_empty() {
            return self.reconcile();
        }
        self.exec(ExecAsk {
            subject,
            program: "",
            argv: &[],
            shell: Some(command),
            profile,
            roots,
            hooks_answer_first: false,
        })
    }

    fn command_on(
        &self,
        subject: &str,
        grant: &StoredDirectoryGrant,
        program: &str,
        argv: &[String],
    ) -> Result<(), DirectoryRefusal> {
        if grant.rules.is_unreadable() {
            return Err(DirectoryRefusal::Program {
                matcher: "command.rules.unlisted".to_owned(),
                rules: Vec::new(),
                audit_recorded: false,
            });
        }
        if !grant.rules.constrains_commands() {
            return Ok(());
        }
        let question = DirectoryCommandQuestion {
            capabilities: capability_names(&grant.capabilities),
            rules: &grant.rules,
            program,
            argv: join_argv(argv),
        };
        match self
            .policy
            .command(&question)
            .map_err(|_| DirectoryRefusal::PolicyUnavailable)?
        {
            PolicyAnswer::Allow => Ok(()),
            PolicyAnswer::Deny { rules } => {
                let matcher = rules
                    .iter()
                    .find(|id| id.starts_with("command.rules."))
                    .cloned()
                    .unwrap_or_else(|| "command.rules.unlisted".to_owned());
                let resource = self.resource_of(grant).ok();
                let recorded = self.deliver_or_keep(&DirectoryAuditRecord {
                    cause: "command-refused".to_owned(),
                    resource_id: Some(grant.resource_id.clone()),
                    environment: resource
                        .as_ref()
                        .map(|item| item.environment.clone())
                        .unwrap_or_default(),
                    canonical_path: resource
                        .as_ref()
                        .map(|item| item.canonical_path.clone())
                        .unwrap_or_default(),
                    initiator: subject.to_owned(),
                    before: Some(matcher.clone()),
                    after: Some(attempted_command(program, argv)),
                    profile: (!rules.is_empty()).then(|| rules.join(",")),
                    at_ms: now_ms(),
                });
                Err(DirectoryRefusal::Program {
                    matcher,
                    rules,
                    audit_recorded: recorded,
                })
            }
        }
    }

    /// A call the harness made without asking. Recorded when the subject has a grant.
    pub fn report_unasked(&self, subject: &str, tool: &str) -> Result<(), DirectoryRefusal> {
        let owed = self.reconcile().err();
        let grants = self.store.grants_for_subject(subject)?;
        let Some(grant) = grants.first() else {
            return owed.map_or(Ok(()), Err);
        };
        let resource = self.resource_of(grant)?;
        let flush = self.flush_pending(&grant.resource_id).err();
        self.audit_required(DirectoryAuditRecord {
            cause: "outside-nessa-gate".to_owned(),
            resource_id: Some(grant.resource_id.clone()),
            environment: resource.environment,
            canonical_path: resource.canonical_path,
            initiator: subject.to_owned(),
            before: None,
            after: Some(tool.to_owned()),
            profile: None,
            at_ms: now_ms(),
        })?;
        owed.or(flush).map_or(Ok(()), Err)
    }

    fn open_root(
        &self,
        resource: &StoredDirectoryResource,
        initiator: &str,
    ) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        match self.files.open_session(&resource.canonical_path) {
            Ok(session) => Ok(session),
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => Err(self
                .mismatch(resource, "marker-missing", initiator)
                .expect_err("a missing directory revokes its grants")),
            Err(error) => Err(error),
        }
    }

    /// Deliver owed mismatch audits, other pending directory audits, and marker
    /// cleanups. Share, read, write, exec, and process start call this.
    pub fn reconcile(&self) -> Result<(), DirectoryRefusal> {
        for (environment, canonical) in self.store.pending_cleanups()? {
            self.finish_cleanup(&environment, &canonical)?;
        }
        for resource_id in self.store.open_audits_pending()? {
            self.retry_open_audit(&resource_id)?;
        }
        for resource_id in self.store.pending_resources()? {
            self.flush_pending(&resource_id)?;
        }
        Ok(())
    }

    /// Write an open audit the sink previously refused, keeping the original initiator.
    pub fn retry_open_audit(&self, resource_id: &str) -> Result<(), DirectoryRefusal> {
        let Some(pending) = self.store.pending_open_audit(resource_id)? else {
            return Ok(());
        };
        let resource = self.store.resource(resource_id)?;
        self.audit
            .record(&DirectoryAuditRecord {
                cause: pending.cause,
                resource_id: Some(resource_id.to_owned()),
                environment: resource
                    .as_ref()
                    .map(|item| item.environment.clone())
                    .unwrap_or_default(),
                canonical_path: resource.map(|item| item.canonical_path).unwrap_or_default(),
                initiator: pending.initiator,
                before: Some("active".to_owned()),
                after: Some("absent".to_owned()),
                profile: None,
                at_ms: now_ms(),
            })
            .map_err(|_| DirectoryRefusal::Store)?;
        self.store.mark_open_audit(resource_id, pending.epoch)
    }

    fn share_existing(
        &self,
        request: ShareDirectory,
        canonical: String,
        bytes: [u8; BINDING_LEN],
    ) -> Result<SharedDirectory, DirectoryRefusal> {
        if let Some(resource) = self.store.resource_at(&request.environment, &canonical)? {
            if resource.binding != bytes {
                return self.mismatch(&resource, "marker-different", &request.initiator);
            }
            self.flush_pending(&resource.resource_id)?;
            return self.insert_subject(request, resource);
        }
        if self.store.resource_by_binding(&bytes)?.is_some() {
            return Err(DirectoryRefusal::BindingCopied);
        }
        let resource = StoredDirectoryResource {
            resource_id: new_id(),
            environment: request.environment.clone(),
            canonical_path: canonical,
            binding: bytes,
        };
        self.record_resource(&resource, &request.initiator, "adopt")?;
        self.insert_subject(request, resource)
    }

    fn share_new(
        &self,
        request: ShareDirectory,
        canonical: String,
    ) -> Result<SharedDirectory, DirectoryRefusal> {
        let binding = self.binding.mint();
        self.files.create_marker(&canonical, &binding)?;
        let resource = StoredDirectoryResource {
            resource_id: new_id(),
            environment: request.environment.clone(),
            canonical_path: canonical.clone(),
            binding,
        };
        if let Err(error) = self.store.insert_resource(
            &resource,
            &self.transition(&resource, &request.initiator, "first-share"),
        ) {
            self.store.record_cleanup(
                &request.environment,
                &canonical,
                &binding,
                &request.initiator,
            )?;
            self.record_file_absent(
                &request.environment,
                &canonical,
                &binding,
                &request.initiator,
                false,
                false,
            )?;
            return Err(error);
        }
        self.flush_pending(&resource.resource_id)?;
        self.insert_subject(request, resource)
    }

    fn insert_subject(
        &self,
        request: ShareDirectory,
        resource: StoredDirectoryResource,
    ) -> Result<SharedDirectory, DirectoryRefusal> {
        let capabilities = request.capabilities.as_str();
        let grant = StoredDirectoryGrant {
            resource_id: resource.resource_id.clone(),
            principal_id: request.subject.principal_id,
            receiver_id: request.subject.receiver_id,
            capabilities: capabilities.clone(),
            rules: request.rules,
        };
        self.store.insert_grant(&grant, &request.initiator)?;
        Ok(SharedDirectory {
            resource_id: resource.resource_id,
            canonical_path: resource.canonical_path,
            capabilities,
        })
    }

    fn mismatch(
        &self,
        resource: &StoredDirectoryResource,
        cause: &str,
        initiator: &str,
    ) -> Result<SharedDirectory, DirectoryRefusal> {
        let revoked = self
            .store
            .revoke_mismatch(&resource.resource_id, cause, initiator)?;
        let recorded = self
            .audit
            .record(&DirectoryAuditRecord {
                cause: revoked.cause.clone(),
                resource_id: Some(resource.resource_id.clone()),
                environment: resource.environment.clone(),
                canonical_path: resource.canonical_path.clone(),
                initiator: revoked.initiator.clone(),
                before: Some("active".to_owned()),
                after: Some("absent".to_owned()),
                profile: None,
                at_ms: now_ms(),
            })
            .is_ok();
        if recorded {
            let _ = self
                .store
                .mark_open_audit(&resource.resource_id, revoked.epoch);
        }
        let _ = self.flush_pending(&resource.resource_id);
        Err(DirectoryRefusal::MarkerMismatch {
            cause: revoked.cause,
            audit_recorded: recorded,
        })
    }

    fn prepare(
        &self,
        subject: &str,
        relative: &str,
        capability: &str,
    ) -> Result<
        (
            StoredDirectoryGrant,
            StoredDirectoryResource,
            Box<dyn DirectorySession>,
        ),
        DirectoryRefusal,
    > {
        let owed = self.reconcile().err();
        let opened = self.open_for_use(subject, relative, capability);
        match opened {
            Ok(value) => owed.map_or(Ok(value), Err),
            Err(error) => Err(error),
        }
    }

    fn open_for_use(
        &self,
        subject: &str,
        relative: &str,
        capability: &str,
    ) -> Result<
        (
            StoredDirectoryGrant,
            StoredDirectoryResource,
            Box<dyn DirectorySession>,
        ),
        DirectoryRefusal,
    > {
        let grants = self.store.grants_for_subject(subject)?;
        let Some(grant) = grants.into_iter().next() else {
            return Err(DirectoryRefusal::NotFound);
        };
        let resource = self.resource_of(&grant)?;
        let session = self.open_root(&resource, subject)?;
        self.open_marker_on(session.as_ref(), &resource, subject)?;
        let facts = facts_for(session.as_ref(), relative, capability)?;
        self.refuse_walk(&grant, &resource, facts, subject, relative, capability)?;
        Ok((grant, resource, session))
    }

    fn refuse_walk(
        &self,
        grant: &StoredDirectoryGrant,
        resource: &StoredDirectoryResource,
        facts: WalkFacts,
        subject: &str,
        relative: &str,
        capability: &str,
    ) -> Result<(), DirectoryRefusal> {
        let question = UseQuestion {
            capabilities: capability_names(&grant.capabilities),
            resource: resource.resource_id.clone(),
            action: format!("directory.{capability}"),
            leave: facts.leave,
            marker: facts.marker,
            _borrow: std::marker::PhantomData,
        };
        match self
            .policy
            .use_directory(&question)
            .map_err(|_| DirectoryRefusal::PolicyUnavailable)?
        {
            PolicyAnswer::Allow => Ok(()),
            PolicyAnswer::Deny { rules } => {
                let (cause, kind) =
                    if facts.marker || rules.iter().any(|id| id == "directory.marker") {
                        ("marker-name", RefusalKind::Marker)
                    } else if facts.leave != PathLeave::Inside
                        || rules.iter().any(|id| id == "directory.path")
                    {
                        ("path-leaves", RefusalKind::Path)
                    } else {
                        ("capability", RefusalKind::Capability)
                    };
                let audit_recorded =
                    self.record_refusal(resource, subject, relative, capability, cause);
                Err(match kind {
                    RefusalKind::Marker => DirectoryRefusal::MarkerName { audit_recorded },
                    RefusalKind::Path => DirectoryRefusal::Outside {
                        leave: facts.leave,
                        audit_recorded,
                    },
                    RefusalKind::Capability => DirectoryRefusal::Capability {
                        rules,
                        audit_recorded,
                    },
                })
            }
        }
    }

    fn record_refusal(
        &self,
        resource: &StoredDirectoryResource,
        subject: &str,
        relative: &str,
        capability: &str,
        cause: &str,
    ) -> bool {
        let record = DirectoryAuditRecord {
            cause: cause.to_owned(),
            resource_id: Some(resource.resource_id.clone()),
            environment: resource.environment.clone(),
            canonical_path: resource.canonical_path.clone(),
            initiator: subject.to_owned(),
            before: Some(format!("directory.{capability}")),
            after: Some(relative.to_owned()),
            profile: None,
            at_ms: now_ms(),
        };
        self.deliver_or_keep(&record)
    }

    /// The sink accepted the record. Otherwise it stays pending for the next delivery.
    fn deliver_or_keep(&self, record: &DirectoryAuditRecord) -> bool {
        if self.audit.record(record).is_ok() {
            return true;
        }
        let _ = self.store.save_pending(record);
        false
    }

    fn open_marker_on(
        &self,
        session: &dyn DirectorySession,
        resource: &StoredDirectoryResource,
        initiator: &str,
    ) -> Result<(), DirectoryRefusal> {
        match session.marker()? {
            MarkerBytes::Exact(bytes) if bytes == resource.binding => Ok(()),
            MarkerBytes::Exact(_) => self
                .mismatch(resource, "marker-different", initiator)
                .map(|_| ()),
            MarkerBytes::Absent => self
                .mismatch(resource, "marker-missing", initiator)
                .map(|_| ()),
            MarkerBytes::Other => self
                .mismatch(resource, "marker-size", initiator)
                .map(|_| ()),
            MarkerBytes::Unusable => self
                .mismatch(resource, "marker-type", initiator)
                .map(|_| ()),
        }
    }

    fn resource_of(
        &self,
        grant: &StoredDirectoryGrant,
    ) -> Result<StoredDirectoryResource, DirectoryRefusal> {
        self.store
            .resource(&grant.resource_id)?
            .ok_or(DirectoryRefusal::NotFound)
    }

    fn finish_cleanup(&self, environment: &str, canonical: &str) -> Result<(), DirectoryRefusal> {
        let _lease = self.lease(environment, canonical);
        self.finish_cleanup_held(environment, canonical)
    }

    /// The caller holds this directory's share lease. The cleanup row is read
    /// again under that lease, so a share that already committed is not unlinked.
    fn finish_cleanup_held(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<(), DirectoryRefusal> {
        let Some(pending) = self.store.cleanup(environment, canonical)? else {
            return Ok(());
        };
        self.record_file_absent(
            environment,
            canonical,
            &pending.binding,
            &pending.initiator,
            pending.unlinked,
            pending.audit_done,
        )?;
        if self.store.cleanup(environment, canonical)?.is_some() {
            return Err(DirectoryRefusal::CleanupPending);
        }
        Ok(())
    }

    /// The absence record is written only after the marker unlink has succeeded.
    /// A failed unlink leaves the cleanup row, with the audit still pending.
    fn record_file_absent(
        &self,
        environment: &str,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
        initiator: &str,
        already_unlinked: bool,
        already_audited: bool,
    ) -> Result<(), DirectoryRefusal> {
        let unlinked = already_unlinked
            || self
                .files
                .release_created_marker(canonical, binding)
                .unwrap_or(false);
        let audit_done = unlinked
            && (already_audited
                || self
                    .audit
                    .record(&DirectoryAuditRecord {
                        cause: "file-absent".to_owned(),
                        resource_id: None,
                        environment: environment.to_owned(),
                        canonical_path: canonical.to_owned(),
                        initiator: initiator.to_owned(),
                        before: None,
                        after: Some("absent".to_owned()),
                        profile: None,
                        at_ms: now_ms(),
                    })
                    .is_ok());
        self.store
            .update_cleanup(environment, canonical, unlinked, audit_done)?;
        if unlinked && audit_done {
            self.store.clear_cleanup(environment, canonical)?;
        }
        Ok(())
    }

    /// Whether `subject` has a directory grant whose command list is a constraint.
    pub fn command_constrained(&self, subject: &str) -> Result<bool, DirectoryRefusal> {
        Ok(self
            .store
            .grants_for_subject(subject)?
            .iter()
            .any(|grant| grant.rules.constrains_commands() || grant.rules.is_unreadable()))
    }

    fn record_resource(
        &self,
        resource: &StoredDirectoryResource,
        initiator: &str,
        cause: &str,
    ) -> Result<(), DirectoryRefusal> {
        self.store
            .insert_resource(resource, &self.transition(resource, initiator, cause))?;
        self.flush_pending(&resource.resource_id)
    }

    fn transition(
        &self,
        resource: &StoredDirectoryResource,
        initiator: &str,
        cause: &str,
    ) -> DirectoryAuditRecord {
        DirectoryAuditRecord {
            cause: cause.to_owned(),
            resource_id: Some(resource.resource_id.clone()),
            environment: resource.environment.clone(),
            canonical_path: resource.canonical_path.clone(),
            initiator: initiator.to_owned(),
            before: Some("none".to_owned()),
            after: Some("active".to_owned()),
            profile: None,
            at_ms: now_ms(),
        }
    }

    fn audit_required(&self, record: DirectoryAuditRecord) -> Result<(), DirectoryRefusal> {
        if self.audit.record(&record).is_ok() {
            return Ok(());
        }
        let resource_id = record.resource_id.clone().unwrap_or_default();
        self.store.save_pending(&record)?;
        let _ = resource_id;
        Err(DirectoryRefusal::Audit)
    }

    fn flush_pending(&self, resource_id: &str) -> Result<(), DirectoryRefusal> {
        for pending in self.store.pending_for(resource_id)? {
            if self.audit.record(&pending.record).is_err() {
                return Err(DirectoryRefusal::Audit);
            }
            self.store.drop_pending(pending.sequence)?;
        }
        Ok(())
    }

    fn lease(&self, environment: &str, canonical: &str) -> LeaseGuard {
        let key = format!("{environment}\0{canonical}");
        let mut leases = self.leases.lock().expect("leases");
        let lease = leases
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Lease {
                    held: std::sync::Mutex::new(false),
                    free: std::sync::Condvar::new(),
                })
            })
            .clone();
        drop(leases);
        let mut held = lease.held.lock().expect("lease");
        while *held {
            held = lease.free.wait(held).expect("lease");
        }
        *held = true;
        drop(held);
        LeaseGuard { lease }
    }
}

struct LeaseGuard {
    lease: Arc<Lease>,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let mut held = self.lease.held.lock().expect("lease");
        *held = false;
        self.lease.free.notify_one();
    }
}

fn facts_for(
    session: &dyn DirectorySession,
    relative: &str,
    capability: &str,
) -> Result<WalkFacts, DirectoryRefusal> {
    let facts = session.walk(relative)?;
    if capability != "write" || facts.leave != PathLeave::Missing {
        return Ok(facts);
    }
    let parent = parent_relative(relative);
    let parent_facts = if parent.is_empty() {
        WalkFacts {
            leave: PathLeave::Inside,
            marker: false,
        }
    } else {
        session.walk(&parent)?
    };
    if parent_facts.leave == PathLeave::Inside && !parent_facts.marker {
        return Ok(WalkFacts {
            leave: PathLeave::Inside,
            marker: false,
        });
    }
    Ok(facts)
}

fn parent_relative(relative: &str) -> String {
    let mut parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.len() <= 1 {
        return String::new();
    }
    parts.pop();
    parts.join("/")
}

fn attempted_command(program: &str, argv: &[String]) -> String {
    if argv.is_empty() {
        return program.to_owned();
    }
    format!("{program}\u{1f}{}", join_argv(argv))
}

fn capability_names(spelling: &str) -> Vec<String> {
    spelling.split('+').map(str::to_owned).collect()
}

fn now_ms() -> u64 {
    wall_now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

fn new_id() -> String {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).expect("resource id");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Builders the tests and the owner use.
pub fn allow_program(program: &str) -> Result<CommandMatcher, DirectoryShapeError> {
    CommandMatcher::new(RuleEffect::Allow, program, None)
}

/// A deny matcher with an argv pattern.
pub fn deny_argv(program: &str, argv: &str) -> Result<CommandMatcher, DirectoryShapeError> {
    CommandMatcher::new(RuleEffect::Deny, program, Some(ArgvPattern::new(argv)?))
}

#[cfg(test)]
#[path = "../../tests/conversation/directory_grant.rs"]
mod tests;
