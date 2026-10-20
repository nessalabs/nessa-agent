//! Stage a verified cut into quarantine, accept it onto an empty live path,
//! and open one deferred conversation.
//!
//! ```text
//! stage ──▶ policy ──▶ decision audit ──▶ verify_bundle
//!             ──▶ prepare files ──▶ seal ──▶ rename partial to quarantine
//!             ──▶ outcome audit (failure deletes the quarantine)
//! accept ──▶ policy ──▶ decision audit ──▶ seal and receipts
//!             ──▶ rename quarantine to live ──▶ outcome audit
//! open ──▶ policy ──▶ decision audit ──▶ write temp session
//!             ──▶ fingerprint ──▶ rename into sessions/
//! ```
//!
//! Arrows are the order one call runs. The live path is not written before
//! accept's rename. This module has no harness, command, or lease issuer.

use std::path::{Path, PathBuf};

use super::ports::{
    DeferredSessionSource, FetchProgress, InventoryDecision, InventoryDenial, LeaseReceipts,
    RestoreAudit, RestoreEvidence, RestoreFacts, RestorePolicyEvaluator, SessionSourceError,
};
use super::verification::{
    verify_bundle, ManifestMacVerifier, VerificationFailure, VerificationLimits, VerificationSource,
};
use super::verification_format::VerificationFormat;
use crate::backup::domain::{
    decode_deferred, deferred_for_open, deferred_session_name, encode_journal,
    extra_session_allowed, paths_are_distinct, posture, read_journal, receipt_is_usable,
    relative_path_is_safe, DeferredLine, DeletionLog, Fingerprint, Freshness, Journal, Posture,
    RestoreFault, SealedFile,
};

/// Filesystem effects of a quarantine. Tests substitute a layout that fails.
pub trait QuarantineLayout {
    fn begin(&self, quarantine: &Path, live: &Path) -> Result<Box<dyn Staging>, RestoreFault>;
    fn load(&self, root: &Path) -> Result<LoadedTree, RestoreFault>;
    fn promote(&self, quarantine: &Path, live: &Path) -> Result<(), RestoreFault>;
    fn remove_tree(&self, path: &Path) -> Result<(), RestoreFault>;
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, RestoreFault>;
    fn session_temp(&self, root: &Path, relative: &str) -> Result<PathBuf, RestoreFault>;
    fn publish_file(&self, temp: &Path, final_path: &Path) -> Result<(), RestoreFault>;
    fn remove_file(&self, path: &Path) -> Result<(), RestoreFault>;
    fn exists(&self, path: &Path) -> Result<bool, RestoreFault>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, RestoreFault>;
}

/// A partial directory. Dropping it without [`Staging::finish`] removes it.
pub trait Staging {
    fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), RestoreFault>;
    fn finish(self: Box<Self>, journal: &str) -> Result<(), RestoreFault>;
}

/// What [`QuarantineLayout::load`] observed. `files` are the sealed set.
pub struct LoadedTree {
    pub journal: String,
    pub files: Vec<SealedFile>,
    pub extras: Vec<String>,
}

/// Turns a verified bundle into the files quarantine will seal.
pub trait CutMaterializer {
    fn prepare(
        &self,
        bundle: &dyn VerificationSource,
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
) -> Result<StageReport, RestoreFault> {
    let target = request.quarantine;
    let kind = request.facts.kind;
    let policies = decide(policy, &request.facts, audit, target)?;
    let recorded = Recorded {
        audit,
        target,
        initiator: kind,
        policies: &policies,
    };
    if let Err(error) = paths_are_distinct(request.quarantine, request.live) {
        return Err(recorded.stop(error));
    }
    if let Err(error) = verify_bundle(
        request.format,
        request.bundle,
        request.verifier,
        request.limits,
    ) {
        return Err(recorded.stop(verification_fault(error)));
    }
    let later = match request.later_inventory {
        Some(text) => match later_reader.read_later(text) {
            Ok(log) => Some(log),
            Err(error) => return Err(recorded.stop(error)),
        },
        None => None,
    };
    let prepared = match materializer.prepare(
        request.bundle,
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
    let mut staging = match layout.begin(request.quarantine, request.live) {
        Ok(staging) => staging,
        Err(error) => return Err(recorded.stop(error)),
    };
    for (path, bytes) in &prepared.files {
        if let Err(error) = staging.write(path, bytes) {
            drop(staging);
            return Err(recorded.stop(error));
        }
    }
    if let Err(error) = staging.finish(&journal) {
        let _ = layout.remove_tree(request.quarantine);
        return Err(recorded.stop(error));
    }
    if recorded
        .write("absent", "verified", "sealed", "effect")
        .is_err()
    {
        let _ = layout.remove_tree(request.quarantine);
        let _ = recorded.write("verified", "absent", RestoreFault::Audit.cause(), "effect");
        return Err(RestoreFault::Audit);
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
/// [`RestoreFault::Audit`] and leaves the tree at the live path.
pub fn accept_quarantine(
    request: &AcceptRequest<'_>,
    policy: &dyn RestorePolicyEvaluator,
    audit: &dyn RestoreAudit,
    layout: &dyn QuarantineLayout,
) -> Result<AcceptReport, RestoreFault> {
    let target = request.quarantine;
    let kind = request.facts.kind;
    let policies = decide(policy, &request.facts, audit, target)?;
    let recorded = Recorded {
        audit,
        target,
        initiator: kind,
        policies: &policies,
    };
    if let Err(error) = paths_are_distinct(request.quarantine, request.live) {
        return Err(recorded.stop(error));
    }
    let loaded = match layout.load(request.quarantine) {
        Ok(loaded) => loaded,
        Err(error) => return Err(recorded.stop(error)),
    };
    let journal = match read_journal(&loaded.journal) {
        Ok(journal) => journal,
        Err(error) => return Err(recorded.stop(error)),
    };
    let deferred = match deferred_lines(layout, request.quarantine) {
        Ok(lines) => lines,
        Err(error) => return Err(recorded.stop(error)),
    };
    if let Err(error) = seal_matches(&journal, &loaded, &deferred) {
        return Err(recorded.stop(error));
    }
    if let Err(error) = receipts_cover(&journal, request.receipts) {
        return Err(recorded.stop(error));
    }
    match layout.exists(request.live) {
        Ok(true) => {
            return Err(recorded.stop(RestoreFault::LiveOccupied));
        }
        Ok(false) => {}
        Err(error) => return Err(recorded.stop(error)),
    }
    if let Err(error) = layout.promote(request.quarantine, request.live) {
        return Err(recorded.stop(error));
    }
    if recorded
        .at(request.live)
        .write("verified", "live", "accepted", "effect")
        .is_err()
    {
        return Err(RestoreFault::Audit);
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
    progress: &mut dyn FetchProgress,
) -> Result<OpenReport, RestoreFault> {
    let target = request.root;
    let kind = request.facts.kind;
    let policies = decide(policy, &request.facts, audit, target)?;
    let recorded = Recorded {
        audit,
        target,
        initiator: kind,
        policies: &policies,
    };
    let loaded = match layout.load(request.root) {
        Ok(loaded) => loaded,
        Err(error) => return Err(recorded.stop(error)),
    };
    if read_journal(&loaded.journal).is_err() {
        return Err(recorded.stop(RestoreFault::Tamper));
    }
    let lines = match deferred_lines(layout, request.root) {
        Ok(lines) => lines,
        Err(error) => return Err(recorded.stop(error)),
    };
    let expected = match deferred_for_open(&lines, request.conversation_id) {
        Ok(line) => line,
        Err(error) => return Err(recorded.stop(error)),
    };
    let relative = match deferred_session_name(request.conversation_id) {
        Ok(relative) => relative,
        Err(error) => return Err(recorded.stop(error)),
    };
    let final_path = match join_relative(request.root, &relative) {
        Ok(path) => path,
        Err(error) => return Err(recorded.stop(error)),
    };
    let present = match layout.exists(&final_path) {
        Ok(present) => present,
        Err(error) => return Err(recorded.stop(error)),
    };
    if present {
        return match source.inspect(request.conversation_id, &final_path) {
            Ok(cut) if cut.size == expected.size && cut.digest == expected.digest => {
                recorded.write("available", "available", "already", "effect")?;
                Ok(OpenReport {
                    size: cut.size,
                    digest: cut.digest,
                    already_present: true,
                })
            }
            Ok(_) => Err(recorded.stop(RestoreFault::SessionDiffers)),
            Err(error) => Err(recorded.stop(source_fault(error))),
        };
    }
    let temp = match layout.session_temp(request.root, &relative) {
        Ok(temp) => temp,
        Err(error) => return Err(recorded.stop(error)),
    };
    if let Err(error) = layout.remove_file(&temp) {
        return Err(recorded.stop(error));
    }
    if let Err(error) = source.write_session(request.conversation_id, &temp, progress) {
        let _ = layout.remove_file(&temp);
        return Err(recorded.stop(source_fault(error)));
    }
    let cut = match source.inspect(request.conversation_id, &temp) {
        Ok(cut) => cut,
        Err(error) => {
            let _ = layout.remove_file(&temp);
            return Err(recorded.stop(source_fault(error)));
        }
    };
    if cut.size != expected.size || cut.digest != expected.digest {
        let _ = layout.remove_file(&temp);
        return Err(recorded.stop(RestoreFault::SourceChanged));
    }
    if recorded
        .write("deferred", "available", "fetched", "effect")
        .is_err()
    {
        let _ = layout.remove_file(&temp);
        return Err(RestoreFault::Audit);
    }
    if let Err(error) = layout.publish_file(&temp, &final_path) {
        let _ = layout.remove_file(&temp);
        return Err(recorded.stop(error));
    }
    Ok(OpenReport {
        size: cut.size,
        digest: cut.digest,
        already_present: false,
    })
}

/// Quarantined until the tree sits on the live path. Replay stays a sealed file.
pub fn root_posture(
    root: &Path,
    live: &Path,
    layout: &dyn QuarantineLayout,
) -> Result<Posture, RestoreFault> {
    let loaded = layout.load(root)?;
    let _journal = read_journal(&loaded.journal)?;
    let root = layout.canonicalize(root)?;
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
    root: &Path,
) -> Result<Vec<DeferredLine>, RestoreFault> {
    let bytes = layout.read_bytes(&root.join("deferred.txt"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| RestoreFault::Tamper)?;
    decode_deferred(text)
}

fn receipts_cover(journal: &Journal, receipts: &dyn LeaseReceipts) -> Result<(), RestoreFault> {
    for lease in &journal.leases {
        match receipts.receipt(lease) {
            Some(text) if receipt_is_usable(&text) => {}
            _ => return Err(RestoreFault::ExecutorsUnfenced),
        }
    }
    Ok(())
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

fn decide(
    policy: &dyn RestorePolicyEvaluator,
    facts: &RestoreFacts<'_>,
    audit: &dyn RestoreAudit,
    target: &Path,
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
    }
    .write("absent", "absent", "decision", name)?;
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
    target: &'a Path,
    initiator: &'a str,
    policies: &'a [String],
}

impl Recorded<'_> {
    fn at<'a>(&'a self, target: &'a Path) -> Recorded<'a> {
        Recorded { target, ..*self }
    }

    fn stop(&self, error: RestoreFault) -> RestoreFault {
        let _ = self.write("unchanged", "unchanged", error.cause(), "effect");
        error
    }

    fn write(
        &self,
        before: &str,
        after: &str,
        cause: &str,
        decision: &str,
    ) -> Result<(), RestoreFault> {
        let target = self.target.display().to_string();
        self.audit
            .append(&RestoreEvidence {
                target: &target,
                before,
                after,
                cause,
                initiator: self.initiator,
                decision,
                policies: self.policies,
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
