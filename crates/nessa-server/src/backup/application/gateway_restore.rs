//! The gateway's view of a quarantined restore.
//!
//! Stage and accept stay the restore use case. This module decides whether
//! that result is an authority the gateway may serve, and opens one deferred
//! conversation only after it is. Historical outbox bytes are not commands.
//! A re-pair identity imports no credential bodies.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::ports::{
    DeferredSessionSource, FetchProgress, PublicationIds, RestoreAudit, RestoreFacts,
    RestorePolicyEvaluator, SessionSourceError,
};
use super::restore::{open_deferred_conversation, OpenReport, OpenRequest, QuarantineLayout};
use crate::backup::domain::{
    audit_path, commands_after_restore, decode_accepted_restore, decode_deferred,
    deferred_conversation_ids, historical_commands_are_blocked, read_journal,
    AcceptedRestorePointer, Fingerprint, RestoreFault, IDENTITY_BODY,
};

/// Why opening a deferred conversation did not publish it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeferredOpenFailure {
    /// The recorded source file is gone.
    SourceMissing,
    /// The source no longer matches the recorded version.
    SourceChanged,
    /// The restore could not be read or published.
    Unavailable,
}

/// What `conversation.read` asks before it opens an agent.
pub trait DeferredConversationOpen: Send + Sync {
    /// Fetch a deferred conversation, or succeed when it is not deferred.
    ///
    /// A missing or changed source publishes nothing.
    fn open(&self, conversation_id: &str) -> Result<(), DeferredOpenFailure>;
}

/// A session source that has nothing to serve.
pub struct AbsentSessionSource;

impl DeferredSessionSource for AbsentSessionSource {
    fn write_session(
        &self,
        _conversation_id: &str,
        _destination: &mut File,
        _progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError> {
        Err(SessionSourceError::Missing)
    }
}

/// Paths the gateway was pointed at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedBinding {
    pub live: PathBuf,
    pub audit: PathBuf,
    pub source: Option<PathBuf>,
}

impl From<AcceptedRestorePointer> for AcceptedBinding {
    fn from(pointer: AcceptedRestorePointer) -> Self {
        Self {
            live: PathBuf::from(pointer.live),
            audit: PathBuf::from(pointer.audit),
            source: pointer.source.map(PathBuf::from),
        }
    }
}

/// The pointer text, or [`RestoreFault::Tamper`] when it is not one.
pub fn binding_from_pointer(text: &str) -> Result<AcceptedBinding, RestoreFault> {
    decode_accepted_restore(text).map(AcceptedBinding::from)
}

/// The live tree is the accepted authority: its audit trusts this journal,
/// and the identity and replay markers are the ones this gateway serves.
pub fn confirm_accepted(
    live: &Path,
    layout: &dyn QuarantineLayout,
    audit: &dyn RestoreAudit,
) -> Result<(), RestoreFault> {
    let anchored = layout.anchor(live)?;
    let label = audit_path(anchored.path())?;
    let loaded = match layout.load(&anchored) {
        Ok(loaded) => loaded,
        Err(_) => return Err(RestoreFault::NotAccepted),
    };
    if read_journal(&loaded.journal).is_err() {
        return Err(RestoreFault::NotAccepted);
    }
    let digest = Fingerprint::of(loaded.journal.as_bytes()).hex();
    let ready = layout.ready(&anchored)?;
    match audit.is_accepted_live(&label, &digest, ready.as_deref()) {
        Ok(true) => {}
        Ok(false) => return Err(RestoreFault::NotAccepted),
        Err(_) => return Err(RestoreFault::Audit),
    }
    let identity = layout.read_relative(&anchored, "identity.txt")?;
    if identity.as_slice() != IDENTITY_BODY.as_bytes() {
        return Err(RestoreFault::Tamper);
    }
    let replay = layout.read_relative(&anchored, "replay.blocked")?;
    if !historical_commands_are_blocked(&replay) {
        return Err(RestoreFault::Tamper);
    }
    Ok(())
}

/// Deferred conversation ids on an accepted restore, in file order.
pub fn list_accepted_conversations(
    live: &Path,
    layout: &dyn QuarantineLayout,
    audit: &dyn RestoreAudit,
) -> Result<Vec<String>, RestoreFault> {
    confirm_accepted(live, layout, audit)?;
    let anchored = layout.anchor(live)?;
    let bytes = layout.read_relative(&anchored, "deferred.txt")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| RestoreFault::Tamper)?;
    Ok(deferred_conversation_ids(&decode_deferred(text)?))
}

/// Fetch one deferred conversation on an accepted restore.
///
/// A conversation that was not deferred is [`RestoreFault::NotDeferred`] and
/// the source is not read. A missing source publishes nothing.
pub fn open_accepted_conversation(
    live: &Path,
    conversation_id: &str,
    facts: RestoreFacts<'_>,
    policy: &dyn RestorePolicyEvaluator,
    audit: &dyn RestoreAudit,
    layout: &dyn QuarantineLayout,
    source: &dyn DeferredSessionSource,
    ids: &dyn PublicationIds,
    progress: &mut dyn FetchProgress,
) -> Result<OpenReport, RestoreFault> {
    confirm_accepted(live, layout, audit)?;
    open_deferred_conversation(
        &OpenRequest {
            facts,
            root: live,
            conversation_id,
        },
        policy,
        audit,
        layout,
        source,
        ids,
        progress,
    )
}

/// Run historical outbox commands only when the restore marker says they are
/// commands. The blocked marker runs nothing. Anything else is refused
/// before the runner is called.
pub fn deliver_historical_commands(
    replay: &[u8],
    commands: &[String],
    run: &mut dyn FnMut(&str),
) -> Result<(), RestoreFault> {
    if !historical_commands_are_blocked(replay) {
        return Err(RestoreFault::Tamper);
    }
    for command in commands_after_restore(replay, commands) {
        run(command);
    }
    Ok(())
}
