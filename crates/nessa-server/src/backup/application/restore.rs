//! Stage a verified cut into quarantine, accept it onto an empty live path,
//! and open one deferred conversation.
//!
//! ```text
//! stage ──▶ policy ──▶ decision audit ──▶ verify_bundle
//!             ──▶ prepare files ──▶ seal ──▶ rename partial to quarantine
//!             ──▶ outcome audit stores the journal digest
//! accept ──▶ policy ──▶ decision audit ──▶ that digest still names the journal
//!             ──▶ seal and receipts ──▶ rename quarantine to live
//!             (a parent sync that fails after the rename is still live)
//! open ──▶ anchor the root ──▶ decision audit on that path
//!             ──▶ fingerprint ──▶ match audit ──▶ rename into sessions/
//!             ──▶ publication id from the port ──▶ available audit
//!             (failure retires the temp name and the final name;
//!              a partial name that remains is retained, not unchanged)
//! ```
//!
//! Arrows are the order one call runs. The live path is not written before
//! accept's rename. This module has no harness, command, or lease issuer.

use std::fs::File;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::ports::{
    DeferredSessionSource, FetchProgress, InventoryDecision, InventoryDenial, LeaseReceipts,
    PublicationIds, RestoreAction, RestoreAudit, RestoreEvidence, RestoreFacts,
    RestorePolicyEvaluator, SessionCut, SessionSourceError, StagingIds,
};
use super::verification::{
    authenticated_bundle, ManifestMacVerifier, VerificationFailure, VerificationLimits,
    VerificationSource,
};
use super::verification_format::VerificationFormat;
use crate::backup::domain::{
    attested_pair, audit_path, decode_deferred, deferred_for_open, deferred_session_name,
    encode_journal, extra_session_allowed, host_volume, path_identity, paths_conflict_on, posture,
    read_journal, receipt_is_usable, relative_path_is_safe, restore_lock_path, seal_attestation,
    session_sidecar, staging_id_is_usable, DeferredLine, DeletionLog, Fingerprint, Freshness,
    Journal, ManifestListing, Posture, RestoreFault, SealedFile, VolumeCase, VolumeProbe,
};

/// Drop releases the restore-root lock.
pub trait RestoreHold: Send {}

/// A restore root bound to the directory that existed when it was anchored.
///
/// The handle is that directory. Later lock, staging, fetch, and promotion
/// use it. The spelling is the audit label, captured from the handle.
pub struct AnchoredRoot {
    pub(crate) ancestor: File,
    pub(crate) tail: Vec<std::ffi::OsString>,
    spelling: PathBuf,
    probe: VolumeProbe,
}

impl AnchoredRoot {
    pub(crate) fn bind(
        ancestor: File,
        tail: Vec<std::ffi::OsString>,
        spelling: PathBuf,
        probe: VolumeProbe,
    ) -> Self {
        Self {
            ancestor,
            tail,
            spelling,
            probe,
        }
    }

    pub fn path(&self) -> &Path {
        &self.spelling
    }

    pub fn probe(&self) -> VolumeProbe {
        self.probe
    }
}

impl Deref for AnchoredRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.spelling
    }
}

impl AsRef<Path> for AnchoredRoot {
    fn as_ref(&self) -> &Path {
        &self.spelling
    }
}

/// The rename onto the live path has happened.
///
/// `durable` is false when a parent directory could not be synced. The live
/// path owns the tree either way; the rename is not undone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Promoted {
    pub durable: bool,
    /// The source could neither be removed nor confirmed unready. False also
    /// covers a source whose bytes remain after its marker was retired.
    pub source_retirement_failed: bool,
    /// At least one live file could not have its temporary protection released.
    pub publication_restricted: bool,
}

/// The session sidecar name was committed. A failed parent-directory sync is
/// a durability fact after that commit, rather than a publication refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishedSession {
    pub durable: bool,
}

/// One deferred fetch's retained directory and exact reserved/published files.
/// All writes, validation, publication and retirement stay with this owner even
/// when an ancestor or a file name is replaced during the operation.
pub trait SessionAttempt {
    /// A writable handle to the empty inode reserved for this source write.
    fn destination(&mut self) -> Result<File, RestoreFault>;
    /// Read the reserved inode, rather than reopening the source's pathname.
    fn inspect(&self, conversation_id: &str) -> Result<SessionCut, RestoreFault>;
    /// Publish a distinct file only after its copied stream still matches.
    fn publish(&mut self, conversation_id: &str, digest: &Fingerprint) -> Result<(), RestoreFault>;
    /// Store the id beside this owned session, refusing a replaced session.
    fn note_publication(&mut self, publication: &str) -> Result<PublishedSession, RestoreFault>;
    /// Release protection after both names have been bound and checked, before
    /// the final availability audit acknowledges this publication.
    fn release(&mut self) -> Result<(), RestoreFault>;
    /// Retire this attempt's exact bytes and sidecar, leaving replacement
    /// objects alone. Unconfirmed retirement returns [`RestoreFault::Retained`].
    fn retire(&mut self) -> Result<(), RestoreFault>;
}

/// Filesystem effects of a quarantine. Tests substitute a layout that fails.
pub trait QuarantineLayout {
    fn begin(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
    ) -> Result<Box<dyn Staging>, RestoreFault>;
    fn load(&self, root: &AnchoredRoot) -> Result<LoadedTree, RestoreFault>;
    /// [`Promoted`] means the live path owns the tree. An ordinary error is a
    /// refusal before publication; [`RestoreFault::Retained`] also covers an
    /// uncertain post-rename identity and must not be recorded as unchanged.
    fn promote(
        &self,
        quarantine: &AnchoredRoot,
        live: &AnchoredRoot,
        loaded: &LoadedTree,
        generation: &str,
    ) -> Result<Promoted, RestoreFault>;
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault>;
    /// Read `relative` through the directory `root` holds.
    fn read_relative(&self, root: &AnchoredRoot, relative: &str) -> Result<Vec<u8>, RestoreFault> {
        self.read_bytes(&join_relative(root.path(), relative)?)
    }
    /// Reserve one fetch beneath the directory retained by this attempt.
    fn begin_session(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Box<dyn SessionAttempt + '_>, RestoreFault>;
    /// The publication stored beside `session`. `None` means this file has none.
    fn publication(&self, session: &Path) -> Result<Option<String>, RestoreFault>;
    fn publication_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
    ) -> Result<Option<String>, RestoreFault> {
        self.publication(&join_relative(root.path(), relative)?)
    }
    fn exists(&self, path: &Path) -> Result<bool, RestoreFault>;
    /// Whether the root name exists under the directory this anchor holds.
    fn occupied(&self, root: &AnchoredRoot) -> Result<bool, RestoreFault> {
        self.exists(root.path())
    }
    fn exists_relative(&self, root: &AnchoredRoot, relative: &str) -> Result<bool, RestoreFault> {
        self.exists(&join_relative(root.path(), relative)?)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault>;
    /// The directory that existed, held open, plus the names under it.
    fn anchor(&self, path: &Path) -> Result<AnchoredRoot, RestoreFault>;
    /// How the volume that will hold `path`'s final component compares names.
    ///
    /// The default is the host rule and one shared device. The directory adapter
    /// reads the volume that actually contains the parent.
    fn volume(&self, path: &Path) -> Result<VolumeProbe, RestoreFault> {
        let _ = path;
        Ok(VolumeProbe {
            device: 0,
            case: host_volume(),
        })
    }
    /// One lock for this root, held across publication and the audit that records it.
    fn hold(&self, root: &AnchoredRoot) -> Result<Box<dyn RestoreHold>, RestoreFault>;
    /// The staging id stored by [`StagedTreeOwner::mark_ready`], when the marker is intact.
    fn ready(&self, root: &AnchoredRoot) -> Result<Option<String>, RestoreFault>;
    /// Stream size and fingerprint of one session file.
    fn session_cut(&self, path: &Path, conversation_id: &str) -> Result<SessionCut, RestoreFault>;
    fn session_cut_relative(
        &self,
        root: &AnchoredRoot,
        relative: &str,
        conversation_id: &str,
    ) -> Result<SessionCut, RestoreFault> {
        self.session_cut(&join_relative(root.path(), relative)?, conversation_id)
    }
}

/// A partial directory.
///
/// [`Staging::discard`] removes it and reports when the directory remains.
/// Dropping it without discard or [`Staging::finish`] removes it on a best-effort basis.
pub trait Staging {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault>;
    /// Publish the partial directory and retain ownership through readiness or cleanup.
    fn finish(self: Box<Self>, journal: &str) -> Result<Box<dyn StagedTreeOwner>, RestoreFault>;
    /// Remove the partial directory. [`RestoreFault::Retained`] means it is still there.
    fn discard(self: Box<Self>) -> Result<(), RestoreFault>;
}

/// The exact directory and files published by one staging attempt.
/// Readiness and retirement use those retained objects rather than a later
/// lookup of the quarantine path. A named directory left behind is retained.
pub trait StagedTreeOwner {
    /// Confirm the intended quarantine name still identifies the owned directory.
    fn check_binding(&self) -> Result<(), RestoreFault>;
    /// Publish readiness inside the owned directory and verify its final binding.
    fn mark_ready(&mut self, staging_id: &str) -> Result<(), RestoreFault>;
    /// Retire all owned bytes and readiness, leaving substituted objects alone.
    fn retire(&mut self) -> Result<(), RestoreFault>;
}

/// What [`QuarantineLayout::load`] observed. `files` are the sealed set.
/// The directory that was read. Promotion renames this object, not whatever later
/// sits at the same path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeId {
    pub volume: u64,
    pub file: [u8; 16],
}

pub struct LoadedTree {
    pub journal: String,
    pub files: Vec<SealedFile>,
    pub extras: Vec<String>,
    /// Sidecars of a session file. Anything else with a helper suffix is in `files`.
    pub helpers: Vec<String>,
    pub identity: TreeId,
    /// Hash of every sealed file, so a later byte change is not this tree.
    pub content: Fingerprint,
}

/// Turns a verified bundle into the files quarantine will seal.
pub trait CutMaterializer {
    fn prepare(
        &self,
        bundle: &dyn VerificationSource,
        listing: &ManifestListing,
        format: &dyn VerificationFormat,
        limits: VerificationLimits,
        later: Option<&DeletionLog>,
    ) -> Result<PreparedCut, RestoreFault>;
}

/// An authenticated later inventory. The bundle's own inventory is not this.
pub trait SealedInventoryReader {
    fn read_later(&self, text: &str) -> Result<DeletionLog, RestoreFault>;
}

pub struct PreparedCut {
    pub files: Vec<(String, Vec<u8>)>,
    pub leases: Vec<String>,
    pub deferred: Vec<DeferredLine>,
    pub freshness: Freshness,
}

#[derive(Debug)]
pub struct StageReport {
    pub leases: Vec<String>,
    pub freshness: Freshness,
}

#[derive(Debug)]
pub struct AcceptReport {
    pub leases: Vec<String>,
}

#[derive(Debug)]
pub struct OpenReport {
    pub size: u64,
    pub digest: Fingerprint,
    pub already_present: bool,
    /// Sidecar directory-sync result of this publication: `Some(true)` was
    /// confirmed, `Some(false)` failed after commit. Cached opens return `None`.
    pub durable: Option<bool>,
}

pub struct StageRequest<'a> {
    pub facts: RestoreFacts<'a>,
    pub bundle: &'a dyn VerificationSource,
    pub format: &'a dyn VerificationFormat,
    pub verifier: &'a dyn ManifestMacVerifier,
    pub limits: VerificationLimits,
    pub later_inventory: Option<&'a str>,
    pub quarantine: &'a Path,
    pub live: &'a Path,
}

/// Verify `request` and seal it under the quarantine path.
///
/// # Errors
/// A denied policy, a failed audit, a bundle the verifier refuses, a fence
/// rollback, or a tree that could not be sealed. The live path is not created.
pub fn stage_quarantine(
    request: &StageRequest<'_>,
    policy: &dyn RestorePolicyEvaluator,
    audit: &dyn RestoreAudit,
    layout: &dyn QuarantineLayout,
    materializer: &dyn CutMaterializer,
    later_reader: &dyn SealedInventoryReader,
    ids: &dyn StagingIds,
) -> Result<StageReport, RestoreFault> {
    let facts = entry_facts(&request.facts, RestoreAction::Stage);
    let (quarantine, label) = resolve_root(
        layout,
        request.quarantine,
        policy,
        &facts,
        audit,
        "absent",
        "absent",
    )?;
    let (live, _) = resolve_root(
        layout,
        request.live,
        policy,
        &facts,
        audit,
        "absent",
        "absent",
    )?;
    let kind = facts.kind;
    let policies = decide(policy, &facts, audit, &label, "absent", "absent")?;
    let recorded = Recorded {
        audit,
        target: &label,
        initiator: kind,
        policies: &policies,
        journal_digest: "",
        root_generation: "",
    };
    if let Err(error) = roots_are_distinct(&quarantine, &live) {
        return Err(recorded.stop(error));
    }
    let _hold = match hold_roots(layout, &quarantine, &live) {
        Ok(hold) => hold,
        Err(error) => return Err(recorded.stop(error)),
    };
    let authenticated = match authenticated_bundle(
        request.format,
        request.bundle,
        request.verifier,
        request.limits,
    ) {
        Ok(authenticated) => authenticated,
        Err(error) => return Err(recorded.stop(verification_fault(error))),
    };
    let later = match request.later_inventory {
        Some(text) => match later_reader.read_later(text) {
            Ok(log) => Some(log),
            Err(error) => return Err(recorded.stop(error)),
        },
        None => None,
    };
    let prepared = match materializer.prepare(
        request.bundle,
        &authenticated.listing,
        request.format,
        request.limits,
        later.as_ref(),
    ) {
        Ok(prepared) => prepared,
        Err(error) => return Err(recorded.stop(error)),
    };
    let hashes = prepared
        .files
        .iter()
        .map(|(path, bytes)| SealedFile {
            path: path.clone(),
            fingerprint: Fingerprint::of(bytes),
        })
        .collect::<Vec<_>>();
    let journal = match encode_journal(prepared.freshness, &prepared.leases, &hashes) {
        Ok(journal) => journal,
        Err(error) => return Err(recorded.stop(error)),
    };
    let mut staging = match layout.begin(&quarantine, &live) {
        Ok(staging) => staging,
        Err(RestoreFault::Retained) => {
            return Err(retained_stage(&recorded, RestoreFault::Retained));
        }
        Err(error) => return Err(recorded.stop(error)),
    };
    for (path, bytes) in &prepared.files {
        if let Err(error) = staging.write(path, bytes) {
            return Err(abandon(staging, &recorded, error));
        }
    }
    let mut published = match staging.finish(&journal) {
        Ok(owner) => owner,
        // A rename whose cleanup could not delete the tree left it unready.
        // Any other finish error means this attempt did not leave a tree.
        Err(RestoreFault::Retained) => {
            return Err(retained_stage(&recorded, RestoreFault::Retained));
        }
        Err(error) => return Err(recorded.stop(error)),
    };
    let digest = Fingerprint::of(journal.as_bytes()).hex();
    let staging_id = match ids.staging_id() {
        Ok(id) if staging_id_is_usable(&id) => id,
        Ok(_) => {
            return Err(retire_staged(
                published.as_mut(),
                &recorded,
                "absent",
                "",
                RestoreFault::Unreadable,
            ))
        }
        Err(error) => {
            return Err(retire_staged(
                published.as_mut(),
                &recorded,
                "absent",
                "",
                error,
            ))
        }
    };
    if let Err(error) = published.check_binding() {
        return Err(retire_staged(
            published.as_mut(),
            &recorded,
            "absent",
            &staging_id,
            error,
        ));
    }
    let attestation = seal_attestation(&digest, &staging_id);
    if recorded
        .record("absent", "verified", "sealed", "effect", &attestation)
        .is_err()
    {
        return Err(retire_staged(
            published.as_mut(),
            &recorded,
            "absent",
            &staging_id,
            RestoreFault::Audit,
        ));
    }
    if let Err(error) = published.mark_ready(&staging_id) {
        return Err(retire_staged(
            published.as_mut(),
            &recorded,
            "verified",
            &staging_id,
            error,
        ));
    }
    if let Err(error) = published.check_binding() {
        return Err(retire_staged(
            published.as_mut(),
            &recorded,
            "verified",
            &staging_id,
            error,
        ));
    }
    if recorded
        .with_journal(&digest)
        .with_generation(&staging_id)
        .record("verified", "ready", "ready", "effect", "")
        .is_err()
    {
        return Err(retire_staged(
            published.as_mut(),
            &recorded,
            "verified",
            &staging_id,
            RestoreFault::Audit,
        ));
    }
    Ok(StageReport {
        leases: prepared.leases,
        freshness: prepared.freshness,
    })
}

pub struct AcceptRequest<'a> {
    pub facts: RestoreFacts<'a>,
    pub quarantine: &'a Path,
    pub live: &'a Path,
    pub receipts: &'a dyn LeaseReceipts,
}

/// Rename a verified quarantine onto an empty live path.
///
/// # Errors
/// The seal does not match, a receipt is missing, the live path exists, or
/// the rename fails. An audit failure after a successful rename returns
/// [`RestoreFault::Audit`] and leaves the tree at the live path. Once source
/// consumption is stored, any later failure requires a fresh stage before retry.
pub fn accept_quarantine(
    request: &AcceptRequest<'_>,
    policy: &dyn RestorePolicyEvaluator,
    audit: &dyn RestoreAudit,
    layout: &dyn QuarantineLayout,
) -> Result<AcceptReport, RestoreFault> {
    let facts = entry_facts(&request.facts, RestoreAction::Accept);
    let (quarantine, label) = resolve_root(
        layout,
        request.quarantine,
        policy,
        &facts,
        audit,
        "verified",
        "verified",
    )?;
    let (live, live_label) = resolve_root(
        layout,
        request.live,
        policy,
        &facts,
        audit,
        "verified",
        "verified",
    )?;
    let kind = facts.kind;
    let policies = decide(policy, &facts, audit, &label, "verified", "verified")?;
    let recorded = Recorded {
        audit,
        target: &label,
        initiator: kind,
        policies: &policies,
        journal_digest: "",
        root_generation: "",
    };
    if let Err(error) = roots_are_distinct(&quarantine, &live) {
        return Err(recorded.stop(error));
    }
    let _hold = match hold_roots(layout, &quarantine, &live) {
        Ok(hold) => hold,
        Err(error) => return Err(recorded.stop(error)),
    };
    let loaded = match layout.load(&quarantine) {
        Ok(loaded) => loaded,
        Err(error) => return Err(recorded.stop(error)),
    };
    let journal = match read_journal(&loaded.journal) {
        Ok(journal) => journal,
        Err(error) => return Err(recorded.stop(error)),
    };
    let digest = Fingerprint::of(loaded.journal.as_bytes()).hex();
    let staging_id = match audit.sealed(&label, &digest) {
        Ok(Some(id)) => id,
        Ok(None) => return Err(recorded.stop(RestoreFault::Tamper)),
        Err(_) => return Err(RestoreFault::Audit),
    };
    let deferred = match deferred_lines(layout, &quarantine, &journal) {
        Ok(lines) => lines,
        Err(error) => return Err(recorded.stop(error)),
    };
    if let Err(error) = seal_matches(&journal, &loaded, &deferred) {
        return Err(recorded.stop(error));
    }
    if let Err(error) = sessions_match(layout, &quarantine, &deferred, &loaded.extras) {
        return Err(recorded.stop(error));
    }
    match publications_are_available(layout, audit, &quarantine, &deferred, &loaded.extras) {
        Ok(()) => {}
        Err(RestoreFault::Audit) => return Err(RestoreFault::Audit),
        Err(error) => return Err(recorded.stop(error)),
    }
    match layout.ready(&quarantine) {
        Ok(Some(marker)) if marker == staging_id => {}
        Ok(_) => return Err(recorded.stop(RestoreFault::Tamper)),
        Err(error) => return Err(recorded.stop(error)),
    }
    let attestations = match attested(&journal, request.receipts) {
        Ok(text) => text,
        Err(error) => return Err(recorded.stop(error)),
    };
    match layout.occupied(&live) {
        Ok(true) => {
            return Err(recorded.stop(RestoreFault::LiveOccupied));
        }
        Ok(false) => {}
        Err(error) => return Err(recorded.stop(error)),
    }
    // Consume this source generation before publication. Once stored, even a
    // failed live intent or retained source requires a fresh stage to retry.
    recorded.record("verified", "spent", "spent", "effect", &staging_id)?;
    recorded
        .at(&live_label)
        .with_journal(&digest)
        .with_generation(&staging_id)
        .record("verified", "promoting", "accepting", "effect", "")?;
    let promoted = match layout.promote(&quarantine, &live, &loaded, &staging_id) {
        Ok(promoted) => promoted,
        Err(RestoreFault::Retained) => return Err(RestoreFault::Retained),
        Err(error) => return Err(recorded.stop(error)),
    };
    let cause = super::ports::PromotionCause::from_effects(
        promoted.durable,
        promoted.source_retirement_failed,
        promoted.publication_restricted,
    )
    .as_str();
    if recorded
        .at(&live_label)
        .with_journal(&digest)
        .with_generation(&staging_id)
        .record("verified", "live", cause, "effect", &attestations)
        .is_err()
    {
        return Err(RestoreFault::Audit);
    }
    if promoted.source_retirement_failed || promoted.publication_restricted {
        return Err(RestoreFault::Retained);
    }
    Ok(AcceptReport {
        leases: journal.leases,
    })
}

pub struct OpenRequest<'a> {
    pub facts: RestoreFacts<'a>,
    pub root: &'a Path,
    pub conversation_id: &'a str,
}

/// Fetch one deferred conversation into `request.root` when its stream matches.
///
/// # Errors
/// The conversation is not deferred, the source is missing or changed, the
/// read is interrupted, or publishing the temp file fails. Another
/// conversation's session file is not written.
pub fn open_deferred_conversation(
    request: &OpenRequest<'_>,
    policy: &dyn RestorePolicyEvaluator,
    audit: &dyn RestoreAudit,
    layout: &dyn QuarantineLayout,
    source: &dyn DeferredSessionSource,
    ids: &dyn PublicationIds,
    progress: &mut dyn FetchProgress,
) -> Result<OpenReport, RestoreFault> {
    let facts = entry_facts(&request.facts, RestoreAction::Fetch);
    let (root, root_label) = resolve_root(
        layout,
        request.root,
        policy,
        &facts,
        audit,
        "deferred",
        "deferred",
    )?;
    let label = fetch_target(&root_label, request.conversation_id);
    let kind = facts.kind;
    let policies = decide(policy, &facts, audit, &label, "deferred", "deferred")?;
    let recorded = Recorded {
        audit,
        target: &label,
        initiator: kind,
        policies: &policies,
        journal_digest: "",
        root_generation: "",
    };
    let _hold = match layout.hold(&root) {
        Ok(hold) => hold,
        Err(error) => return Err(recorded.stop(error)),
    };
    let loaded = match layout.load(&root) {
        Ok(loaded) => loaded,
        Err(error) => return Err(recorded.stop(error)),
    };
    let journal = match read_journal(&loaded.journal) {
        Ok(journal) => journal,
        Err(_) => return Err(recorded.stop(RestoreFault::Tamper)),
    };
    let digest = Fingerprint::of(loaded.journal.as_bytes()).hex();
    let ready = match layout.ready(&root) {
        Ok(ready) => ready,
        Err(error) => return Err(recorded.stop(error)),
    };
    match audit.trusted_journal(&root_label, &digest, ready.as_deref()) {
        Ok(true) => {}
        Ok(false) => return Err(recorded.stop(RestoreFault::Tamper)),
        Err(_) => return Err(RestoreFault::Audit),
    }
    let lines = match deferred_lines(layout, &root, &journal) {
        Ok(lines) => lines,
        Err(error) => return Err(recorded.stop(error)),
    };
    if let Err(error) = seal_matches(&journal, &loaded, &lines) {
        return Err(recorded.stop(error));
    }
    let expected = match deferred_for_open(&lines, request.conversation_id) {
        Ok(line) => line,
        Err(error) => return Err(recorded.stop(error)),
    };
    let relative = match deferred_session_name(request.conversation_id) {
        Ok(relative) => relative,
        Err(error) => return Err(recorded.stop(error)),
    };
    let present = match layout.exists_relative(&root, &relative) {
        Ok(present) => present,
        Err(error) => return Err(recorded.stop(error)),
    };
    if present {
        let current = match layout.publication_relative(&root, &relative) {
            Ok(current) => current,
            Err(error) => return Err(recorded.stop(error)),
        };
        let audited = match current.as_deref() {
            Some(publication) => match audit.available(request.conversation_id, publication) {
                Ok(audited) => audited,
                Err(_) => return Err(RestoreFault::Audit),
            },
            None => false,
        };
        if let Some(publication) = current.filter(|_| audited) {
            return match layout.session_cut_relative(&root, &relative, request.conversation_id) {
                Ok(cut) if cut.size == expected.size && cut.digest == expected.digest => {
                    recorded.record("available", "available", "already", "effect", &publication)?;
                    Ok(OpenReport {
                        size: cut.size,
                        digest: cut.digest,
                        already_present: true,
                        durable: None,
                    })
                }
                Ok(_) => Err(recorded.stop(RestoreFault::SessionDiffers)),
                Err(error) => Err(recorded.stop(error)),
            };
        }
        // An unaudited existing file is not owned by this attempt.
        return Err(RestoreFault::Retained);
    }
    let mut attempt = match layout.begin_session(&root, &relative) {
        Ok(attempt) => attempt,
        Err(RestoreFault::Retained) => return Err(RestoreFault::Retained),
        Err(error) => return Err(recorded.stop(error)),
    };
    let mut destination = match attempt.destination() {
        Ok(destination) => destination,
        Err(error) => return Err(refuse_attempt(attempt.as_mut(), &recorded, error)),
    };
    let written = source.write_session(request.conversation_id, &mut destination, progress);
    drop(destination);
    if let Err(error) = written {
        return Err(refuse_attempt(
            attempt.as_mut(),
            &recorded,
            source_fault(error),
        ));
    }
    let cut = match attempt.inspect(request.conversation_id) {
        Ok(cut) => cut,
        Err(error) => return Err(refuse_attempt(attempt.as_mut(), &recorded, error)),
    };
    if cut.size != expected.size || cut.digest != expected.digest {
        return Err(refuse_attempt(
            attempt.as_mut(),
            &recorded,
            RestoreFault::SourceChanged,
        ));
    }
    if recorded
        .write("deferred", "held", "matched", "effect")
        .is_err()
    {
        return Err(retire_attempt_or_audit(attempt.as_mut()));
    }
    if let Err(error) = attempt.publish(request.conversation_id, &cut.digest) {
        return Err(refuse_attempt(attempt.as_mut(), &recorded, error));
    }
    let publication = match ids.publication_id() {
        Ok(publication) => publication,
        Err(error) => return Err(refuse_attempt(attempt.as_mut(), &recorded, error)),
    };
    match audit.available(request.conversation_id, &publication) {
        Ok(false) => {}
        Ok(true) => {
            return Err(refuse_attempt(
                attempt.as_mut(),
                &recorded,
                RestoreFault::Tamper,
            ))
        }
        Err(_) => return Err(retire_attempt_or_audit(attempt.as_mut())),
    }
    let committed = match attempt.note_publication(&publication) {
        Ok(committed) => committed,
        Err(error) => return Err(refuse_attempt(attempt.as_mut(), &recorded, error)),
    };
    if let Err(error) = attempt.release() {
        return Err(refuse_attempt(attempt.as_mut(), &recorded, error));
    }
    if recorded
        .record("held", "available", "fetched", "effect", &publication)
        .is_err()
    {
        return Err(retire_attempt_or_audit(attempt.as_mut()));
    }
    Ok(OpenReport {
        size: cut.size,
        digest: cut.digest,
        already_present: false,
        durable: Some(committed.durable),
    })
}

/// Quarantined until the tree sits on the live path. Replay stays a sealed file.
pub fn root_posture(
    root: &Path,
    live: &Path,
    layout: &dyn QuarantineLayout,
) -> Result<Posture, RestoreFault> {
    let anchored = layout.anchor(root)?;
    let loaded = layout.load(&anchored)?;
    let _journal = read_journal(&loaded.journal)?;
    let root = layout.canonicalize(anchored.path())?;
    if !layout.exists(live)? {
        return Ok(Posture::Quarantined);
    }
    Ok(posture(root == layout.canonicalize(live)?))
}

fn seal_matches(
    journal: &Journal,
    loaded: &LoadedTree,
    deferred: &[DeferredLine],
) -> Result<(), RestoreFault> {
    for helper in &loaded.helpers {
        let Some(session) = session_sidecar(helper) else {
            return Err(RestoreFault::Tamper);
        };
        if !extra_session_allowed(&session, deferred) {
            return Err(RestoreFault::Tamper);
        }
    }
    if loaded.files.len() != journal.files.len() {
        return Err(RestoreFault::Tamper);
    }
    for expected in &journal.files {
        let Some(found) = loaded.files.iter().find(|file| file.path == expected.path) else {
            return Err(RestoreFault::Tamper);
        };
        if found.fingerprint != expected.fingerprint {
            return Err(RestoreFault::Tamper);
        }
    }
    for extra in &loaded.extras {
        if !extra_session_allowed(extra, deferred) {
            return Err(RestoreFault::Tamper);
        }
    }
    Ok(())
}

fn deferred_lines(
    layout: &dyn QuarantineLayout,
    root: &AnchoredRoot,
    journal: &Journal,
) -> Result<Vec<DeferredLine>, RestoreFault> {
    let bytes = layout.read_relative(root, "deferred.txt")?;
    let expected = journal
        .files
        .iter()
        .find(|file| file.path == "deferred.txt")
        .ok_or(RestoreFault::Tamper)?;
    if Fingerprint::of(&bytes) != expected.fingerprint {
        return Err(RestoreFault::Tamper);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| RestoreFault::Tamper)?;
    decode_deferred(text)
}

fn attested(journal: &Journal, receipts: &dyn LeaseReceipts) -> Result<String, RestoreFault> {
    let mut text = String::new();
    for lease in &journal.leases {
        match receipts.receipt(lease) {
            Some(receipt) if receipt_is_usable(&receipt) => {
                text.push_str(&attested_pair(lease, &receipt));
            }
            _ => return Err(RestoreFault::ExecutorsUnfenced),
        }
    }
    Ok(text)
}

fn join_relative(root: &Path, relative: &str) -> Result<PathBuf, RestoreFault> {
    if !relative_path_is_safe(relative) {
        return Err(RestoreFault::Tamper);
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    Ok(path)
}

/// A fetched session stays available for its publication after the root is renamed.
fn fetch_target(root_label: &str, conversation_id: &str) -> String {
    format!("{conversation_id}\n{root_label}")
}

/// An extra session is promotable only when its publication was recorded available.
fn publications_are_available(
    layout: &dyn QuarantineLayout,
    audit: &dyn RestoreAudit,
    root: &AnchoredRoot,
    deferred: &[DeferredLine],
    extras: &[String],
) -> Result<(), RestoreFault> {
    for extra in extras {
        let Some(line) = deferred.iter().find(|line| {
            deferred_session_name(&line.conversation_id).is_ok_and(|name| name == *extra)
        }) else {
            return Err(RestoreFault::Tamper);
        };
        let Some(publication) = layout.publication_relative(root, extra)? else {
            return Err(RestoreFault::Tamper);
        };
        match audit.available(&line.conversation_id, &publication) {
            Ok(true) => {}
            Ok(false) => return Err(RestoreFault::Tamper),
            Err(_) => return Err(RestoreFault::Audit),
        }
    }
    Ok(())
}

fn sessions_match(
    layout: &dyn QuarantineLayout,
    root: &AnchoredRoot,
    deferred: &[DeferredLine],
    extras: &[String],
) -> Result<(), RestoreFault> {
    for extra in extras {
        let mut matched = false;
        for line in deferred {
            let Ok(name) = deferred_session_name(&line.conversation_id) else {
                return Err(RestoreFault::Tamper);
            };
            if name != *extra {
                continue;
            }
            let cut = layout
                .session_cut_relative(root, extra, &line.conversation_id)
                .map_err(|_| RestoreFault::Tamper)?;
            if cut.size != line.size || cut.digest != line.digest {
                return Err(RestoreFault::Tamper);
            }
            matched = true;
        }
        if !matched {
            return Err(RestoreFault::Tamper);
        }
    }
    Ok(())
}

struct HeldRoots {
    _earlier: Box<dyn RestoreHold>,
    _later: Box<dyn RestoreHold>,
}

/// The two roots, and each root's lock file, are different identities.
fn roots_are_distinct(quarantine: &AnchoredRoot, live: &AnchoredRoot) -> Result<(), RestoreFault> {
    let left = quarantine.probe();
    let right = live.probe();
    paths_conflict_on(quarantine.path(), live.path(), left, right)?;
    paths_conflict_on(
        &restore_lock_path(quarantine.path())?,
        live.path(),
        left,
        right,
    )?;
    paths_conflict_on(
        &restore_lock_path(live.path())?,
        quarantine.path(),
        right,
        left,
    )?;
    Ok(())
}

/// Keep cleanup with the published objects. An uncertain retirement remains
/// retained even when its audit also fails; no stale path is an authority here.
fn retire_staged(
    owner: &mut dyn StagedTreeOwner,
    recorded: &Recorded<'_>,
    before: &str,
    staging_id: &str,
    error: RestoreFault,
) -> RestoreFault {
    let retired = owner.retire().is_ok();
    let stored = recorded
        .record(
            before,
            if retired { "absent" } else { "unready" },
            error.cause(),
            "effect",
            staging_id,
        )
        .is_ok();
    if !retired {
        RestoreFault::Retained
    } else if !stored {
        RestoreFault::Audit
    } else {
        error
    }
}

fn hold_roots(
    layout: &dyn QuarantineLayout,
    first: &AnchoredRoot,
    second: &AnchoredRoot,
) -> Result<HeldRoots, RestoreFault> {
    let (left, _right) = ordered_roots(
        first.path(),
        first.probe().case,
        second.path(),
        second.probe().case,
    )?;
    let (earlier, later) = if left == first.path() {
        (first, second)
    } else {
        (second, first)
    };
    Ok(HeldRoots {
        _earlier: layout.hold(earlier)?,
        _later: layout.hold(later)?,
    })
}

/// One order for every spelling of the same directories. Raw byte order
/// deadlocks when `a`/`B` and `A`/`b` are one pair on a folding volume.
pub(crate) fn ordered_roots(
    first: &Path,
    first_case: VolumeCase,
    second: &Path,
    second_case: VolumeCase,
) -> Result<(PathBuf, PathBuf), RestoreFault> {
    let left = path_identity(first, first_case)?;
    let right = path_identity(second, second_case)?;
    if left == right {
        return Err(RestoreFault::Paths);
    }
    if left < right {
        Ok((first.to_path_buf(), second.to_path_buf()))
    } else {
        Ok((second.to_path_buf(), first.to_path_buf()))
    }
}

fn resolve_root(
    layout: &dyn QuarantineLayout,
    path: &Path,
    policy: &dyn RestorePolicyEvaluator,
    facts: &RestoreFacts<'_>,
    audit: &dyn RestoreAudit,
    before: &str,
    after: &str,
) -> Result<(AnchoredRoot, String), RestoreFault> {
    // A lossy display of a non-Unicode path can collide with another root.
    // Refuse it before any audit line is stored.
    if audit_path(path).is_err() {
        return Err(RestoreFault::Paths);
    }
    match layout.anchor(path) {
        Ok(anchored) => {
            let label = audit_path(anchored.path())?;
            Ok((anchored, label))
        }
        Err(error) => {
            let label = audit_path(path)?;
            let policies = decide(policy, facts, audit, &label, before, after)?;
            Err(Recorded {
                audit,
                target: &label,
                initiator: facts.kind,
                policies: &policies,
                journal_digest: "",
                root_generation: "",
            }
            .stop(error))
        }
    }
}

/// Outcome shared by retained construction, finish, and discard failures.
/// A rejected audit cannot hide the partial tree that still needs recovery.
fn retained_stage(recorded: &Recorded<'_>, cause: RestoreFault) -> RestoreFault {
    let _ = recorded.write("absent", "unready", cause.cause(), "effect");
    RestoreFault::Retained
}

/// The partial directory is gone, or it is still there and the caller is told retained.
fn abandon(
    staging: Box<dyn Staging>,
    recorded: &Recorded<'_>,
    error: RestoreFault,
) -> RestoreFault {
    match staging.discard() {
        Ok(()) => recorded.stop(error),
        Err(_) => retained_stage(recorded, error),
    }
}

/// A refusal can claim unchanged only after the exact attempt was retired.
fn refuse_attempt(
    attempt: &mut dyn SessionAttempt,
    recorded: &Recorded<'_>,
    error: RestoreFault,
) -> RestoreFault {
    match attempt.retire() {
        Ok(()) => recorded.stop(error),
        Err(_) => RestoreFault::Retained,
    }
}

fn retire_attempt_or_audit(attempt: &mut dyn SessionAttempt) -> RestoreFault {
    if attempt.retire().is_ok() {
        RestoreFault::Audit
    } else {
        RestoreFault::Retained
    }
}

fn entry_facts<'a>(facts: &RestoreFacts<'a>, action: RestoreAction) -> RestoreFacts<'a> {
    RestoreFacts {
        kind: facts.kind,
        cause: facts.cause,
        action,
    }
}

fn decide(
    policy: &dyn RestorePolicyEvaluator,
    facts: &RestoreFacts<'_>,
    audit: &dyn RestoreAudit,
    target: &str,
    before: &str,
    after: &str,
) -> Result<Vec<String>, RestoreFault> {
    let decision = policy.evaluate(facts);
    let (name, policies, denied) = match &decision {
        InventoryDecision::Allow { policies } => ("allow", policies.clone(), false),
        InventoryDecision::Deny(denial) => ("deny", denial_policies(denial), true),
    };
    Recorded {
        audit,
        target,
        initiator: facts.kind,
        policies: &policies,
        journal_digest: "",
        root_generation: "",
    }
    .write(before, after, "decision", name)?;
    if denied {
        return Err(RestoreFault::Denied);
    }
    Ok(policies)
}

fn denial_policies(denial: &InventoryDenial) -> Vec<String> {
    match denial {
        InventoryDenial::Forbidden { policies } => policies.clone(),
        InventoryDenial::NotPermitted | InventoryDenial::Unavailable => Vec::new(),
    }
}

#[derive(Clone, Copy)]
struct Recorded<'a> {
    audit: &'a dyn RestoreAudit,
    target: &'a str,
    initiator: &'a str,
    policies: &'a [String],
    journal_digest: &'a str,
    root_generation: &'a str,
}

impl Recorded<'_> {
    fn with_generation<'a>(&'a self, root_generation: &'a str) -> Recorded<'a> {
        Recorded {
            root_generation,
            ..*self
        }
    }
    fn with_journal<'a>(&'a self, journal_digest: &'a str) -> Recorded<'a> {
        Recorded {
            journal_digest,
            ..*self
        }
    }
    fn at<'a>(&'a self, target: &'a str) -> Recorded<'a> {
        Recorded { target, ..*self }
    }

    fn stop(&self, error: RestoreFault) -> RestoreFault {
        if self
            .write("unchanged", "unchanged", error.cause(), "effect")
            .is_err()
        {
            return RestoreFault::Audit;
        }
        error
    }

    fn write(
        &self,
        before: &str,
        after: &str,
        cause: &str,
        decision: &str,
    ) -> Result<(), RestoreFault> {
        self.record(before, after, cause, decision, "")
    }

    fn record(
        &self,
        before: &str,
        after: &str,
        cause: &str,
        decision: &str,
        attestations: &str,
    ) -> Result<(), RestoreFault> {
        self.audit
            .append(&RestoreEvidence {
                target: self.target,
                before,
                after,
                cause,
                initiator: self.initiator,
                decision,
                policies: self.policies,
                attestations,
                journal_digest: self.journal_digest,
                root_generation: self.root_generation,
            })
            .map_err(|_| RestoreFault::Audit)
    }
}

fn verification_fault(_error: VerificationFailure) -> RestoreFault {
    RestoreFault::Verification
}

fn source_fault(error: SessionSourceError) -> RestoreFault {
    match error {
        SessionSourceError::Missing => RestoreFault::SourceMissing,
        SessionSourceError::Interrupted => RestoreFault::Interrupted,
        SessionSourceError::Unreadable => RestoreFault::Unreadable,
    }
}
