// Each mutation removes one real safety boundary from its owning finite model.
// Literal replacements fail closed when a model changes; they never weaken a
// different action silently or substitute source-code execution for model evidence.

function replaceExactlyOnce(spec, before, after) {
  const first = spec.indexOf(before)
  if (first < 0 || spec.indexOf(before, first + before.length) >= 0)
    throw new Error("The expected unique runtime-model action was not found")
  return spec.slice(0, first) + after + spec.slice(first + before.length)
}

export const probes = [
  {
    document: "sdk-attachment",
    name: "Publish a fenced attachment candidate as ready",
    expectedRules: ["ReadyNeedsCurrent"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'PublishAck == phase = "Published" /\\ generation = 0 /\\ phase\' = "Attached" /\\ ready\' = TRUE /\\ audit\' = "Acknowledged" /\\ UNCHANGED <<generation, created>>',
        'PublishAck == phase = "Published" /\\ phase\' = "Attached" /\\ ready\' = TRUE /\\ audit\' = "Acknowledged" /\\ UNCHANGED <<generation, created>>',
      ),
  },
  {
    document: "sdk-scheduling",
    name: "Dispatch accepted input before save acknowledgement",
    expectedRules: ["DispatchNeedsDurability"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Dispatch == phase = "Queued" /\\ saved /\\ phase\' = "Running" /\\ dispatches\' = 1 /\\ UNCHANGED <<saved, retired, fallback, receipt, retryIdentity, recovered>>',
        'Dispatch == phase = "Queued" /\\ phase\' = "Running" /\\ dispatches\' = 1 /\\ UNCHANGED <<saved, retired, fallback, receipt, retryIdentity, recovered>>',
      ),
  },
  {
    document: "sdk-reviews",
    name: "Consume a review without its exact validation",
    expectedRules: ["ExactReview"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Select == phase = "Pending" /\\ valid /\\ phase\' = "Selected" /\\ selected\' = TRUE /\\ UNCHANGED <<selectionAudit, writes, valid>>',
        'Select == phase = "Pending" /\\ phase\' = "Selected" /\\ selected\' = TRUE /\\ UNCHANGED <<selectionAudit, writes, valid>>',
      ),
  },
  {
    document: "sdk-history",
    name: "Acknowledge a save before its physical fact exists",
    expectedRules: ["NoInventedCommit"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Commit == phase = "Saving" /\\ physical = 1 /\\ binding /\\ phase\' = "Acknowledged" /\\ committed\' = 1 /\\ UNCHANGED <<physical, binding, attempts>>',
        'Commit == phase = "Saving" /\\ binding /\\ phase\' = "Acknowledged" /\\ committed\' = 1 /\\ UNCHANGED <<physical, binding, attempts>>',
      ),
  },
  {
    document: "sdk-runtime-usage-limit-continuation",
    name: "Commit a continuation fence after losing its lease",
    expectedRules: ["FenceNeedsLease"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Fence == phase = "Delivering" /\\ lease /\\ phase\' = "Submitting" /\\ fenced\' = TRUE /\\ identity\' = TRUE /\\ UNCHANGED <<lease, attempted>>',
        'Fence == phase = "Delivering" /\\ phase\' = "Submitting" /\\ fenced\' = TRUE /\\ identity\' = TRUE /\\ UNCHANGED <<lease, attempted>>',
      ),
  },
  {
    document: "sdk-runtime-select-a-model-approval-preset-or-reasoning-effort",
    name: "Send before the acknowledged selection is reconciled",
    expectedRules: ["SendNeedsSourceView"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Send == phase = "Applied" /\\ reconciled /\\ phase\' = "Submitted" /\\ UNCHANGED <<selected, active, ack, reconciled, nextTurn>>',
        'Send == phase = "Applied" /\\ phase\' = "Submitted" /\\ UNCHANGED <<selected, active, ack, reconciled, nextTurn>>',
      ),
  },
  {
    document: "auth-admission",
    name: "Admit a session after the access lifetime fails",
    expectedRules: ["AdmissionNeedsCoherentAccess"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Admit == phase = "Reading" /\\ proof /\\ binding /\\ member /\\ live /\\ fresh /\\ phase\' = "Admitted" /\\ checked\' = TRUE /\\ UNCHANGED <<proof, binding, member, live, fresh>>',
        'Admit == phase = "Reading" /\\ proof /\\ binding /\\ member /\\ fresh /\\ phase\' = "Admitted" /\\ checked\' = TRUE /\\ UNCHANGED <<proof, binding, member, live, fresh>>',
      ),
  },
  {
    document: "auth-credential",
    name: "Issue the same credential command and bearer again",
    expectedRules: ["BearerReturnedOnce"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "Issue == ~issued /\\ issued' = TRUE /\\ receipt' = TRUE /\\ secretReturns' = secretReturns + 1 /\\ UNCHANGED <<revoked, firstRevocation, replays>>",
        "Issue == secretReturns < 2 /\\ issued' = TRUE /\\ receipt' = TRUE /\\ secretReturns' = secretReturns + 1 /\\ UNCHANGED <<revoked, firstRevocation, replays>>",
      ),
  },
  {
    document: "client-connection",
    name: "Adopt a replacement already terminated",
    expectedRules: ["NoDeadAdoption"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Adopt == phase = "Reconnecting" /\\ candidate /\\ ~closed /\\ ~terminated /\\ phase\' = "Connected" /\\ candidate\' = FALSE /\\ UNCHANGED <<attempt, terminated, closed>>',
        'Adopt == phase = "Reconnecting" /\\ candidate /\\ ~closed /\\ phase\' = "Connected" /\\ candidate\' = FALSE /\\ UNCHANGED <<attempt, terminated, closed>>',
      ),
  },
  {
    document: "client-commands",
    name: "Settle a new wire request with an old reply",
    expectedRules: ["ReplyNeedsCurrentWire"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Reply == phase = "Waiting" /\\ reply = wire /\\ phase\' = "Acknowledged" /\\ settled\' = TRUE /\\ UNCHANGED <<dispatched, wire, reply>>',
        'Reply == phase = "Waiting" /\\ phase\' = "Acknowledged" /\\ settled\' = TRUE /\\ UNCHANGED <<dispatched, wire, reply>>',
      ),
  },
  {
    document: "mcp-sessions",
    name: "Register an upstream session after grant revocation",
    expectedRules: ["NoOpenAfterRevoke"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Register == phase = "Opening" /\\ grant /\\ phase\' = "Open" /\\ UNCHANGED <<grant, listed, physicalReleased>>',
        'Register == phase = "Opening" /\\ phase\' = "Open" /\\ UNCHANGED <<grant, listed, physicalReleased>>',
      ),
  },
  {
    document: "storage-publication",
    name: "Remove an origin reservation replaced by a newer identity",
    expectedRules: ["NoNewerRemoval"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "Cleanup == cleanupArmed /\\ current # 2 /\\ phase' = \"Discarded\" /\\ removedNewer' = (current = 2) /\\ current' = 0 /\\ cleanupArmed' = FALSE /\\ UNCHANGED <<visible>>",
        "Cleanup == cleanupArmed /\\ phase' = \"Discarded\" /\\ removedNewer' = (current = 2) /\\ current' = 0 /\\ cleanupArmed' = FALSE /\\ UNCHANGED <<visible>>",
      ),
  },
  {
    document: "storage-database",
    name: "Stage an upgrade without integrity validation",
    expectedRules: ["StagedRequiresValidation"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Upgrade == phase = "Locked" /\\ integrity /\\ chain /\\ phase\' = "Staged" /\\ staged\' = 3 /\\ UNCHANGED <<original, version, chain, integrity, committed>>',
        'Upgrade == phase = "Locked" /\\ chain /\\ phase\' = "Staged" /\\ staged\' = 3 /\\ UNCHANGED <<original, version, chain, integrity, committed>>',
      ),
  },
  {
    document: "endpoint-discovery",
    name: "Verify discovery despite a mismatched process identity",
    expectedRules: ["VerifiedIdentity"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Verify == phase = "Probing" /\\ complete /\\ address /\\ instance /\\ process /\\ managed /\\ phase\' = "Verified" /\\ UNCHANGED <<complete, address, instance, process, managed>>',
        'Verify == phase = "Probing" /\\ complete /\\ address /\\ instance /\\ managed /\\ phase\' = "Verified" /\\ UNCHANGED <<complete, address, instance, process, managed>>',
      ),
  },
  {
    document: "sdk-runtime-install-or-replace-a-pinned-agent-runtime",
    name: "Reclaim a runtime while its predecessor use is live",
    expectedRules: ["NoReclaimLiveUse"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Reclaim == phase = "Published" /\\ ~oldUse /\\ reclaimed\' = TRUE /\\ UNCHANGED <<phase, digest, audit, oldUse>>',
        'Reclaim == phase = "Published" /\\ reclaimed\' = TRUE /\\ UNCHANGED <<phase, digest, audit, oldUse>>',
      ),
  },
  {
    document: "tooling-runtime",
    name: "Complete runtime assembly before output verification",
    expectedRules: ["CompleteNeedsOutput"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Complete == phase = "Published" /\\ outputValid /\\ phase\' = "Complete" /\\ UNCHANGED <<oldManifest, verified, outputValid>>',
        'Complete == phase = "Published" /\\ phase\' = "Complete" /\\ UNCHANGED <<oldManifest, verified, outputValid>>',
      ),
  },
  {
    document: "sdk-ownership",
    name: "Complete an ownership close before resource release",
    expectedRules: ["CompletionNeedsRelease"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "Complete == sealed /\\ released /\\ audit /\\ durable /\\ complete' = TRUE /\\ UNCHANGED <<sealed, reserved, started, released, audit, published, durable>>",
        "Complete == sealed /\\ audit /\\ durable /\\ complete' = TRUE /\\ UNCHANGED <<sealed, reserved, started, released, audit, published, durable>>",
      ),
  },
  {
    document: "sdk-runtime-the-first-agent-launch-is-warmed-in-the-background",
    name: "Save warmup completion before its audit acknowledgement",
    expectedRules: ["CompletionNeedsRelease"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Record == phase = "Released" /\\ audit /\\ released /\\ phase\' = "Warmed" /\\ record\' = TRUE /\\ UNCHANGED <<started, released, audit>>',
        'Record == phase = "Released" /\\ released /\\ phase\' = "Warmed" /\\ record\' = TRUE /\\ UNCHANGED <<started, released, audit>>',
      ),
  },
  {
    document: "auth-registry-refusal",
    name: "Reuse unsafe-lock refusal after registry bytes were read",
    expectedRules: ["UnsafeLockBeforeRead"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'UnsafeLock == phase = "Observed" /\\ phase\' = "LockPreserved" /\\ preserved\' = TRUE /\\ originalFault\' = TRUE /\\ UNCHANGED <<registryRead, audit>>',
        "UnsafeLock == TRUE /\\ phase' = \"LockPreserved\" /\\ preserved' = TRUE /\\ originalFault' = TRUE /\\ UNCHANGED <<registryRead, audit>>",
      ),
  },
  {
    document: "sdk-child-preparation",
    name: "Publish a prepared child before its durable acknowledgement",
    expectedRules: ["ReadyDurable"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'PublishReady == phase = "Prepared" /\\ durable /\\ ~sealed /\\ phase\' = "Ready" /\\ UNCHANGED <<audit, durable, submitted, sealed>>',
        'PublishReady == phase = "Prepared" /\\ ~sealed /\\ phase\' = "Ready" /\\ UNCHANGED <<audit, durable, submitted, sealed>>',
      ),
  },
  {
    document: "sdk-history-restoration",
    name: "Resume history without a matching provider identity",
    expectedRules: ["ResumeExactProvider"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Resume == phase = "Prepared" /\\ providerMatch /\\ agreement /\\ phase\' = "Resume" /\\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch, recordOnly>>',
        'Resume == phase = "Prepared" /\\ agreement /\\ phase\' = "Resume" /\\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch, recordOnly>>',
      ),
  },
  {
    document: "sdk-permission-rules",
    name: "Remember a scoped rule without host policy allowance",
    expectedRules: ["RuleRequiresPolicy"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Remember == phase = "Pending" /\\ policy /\\ phase\' = "Remembered" /\\ rule\' = TRUE /\\ ruleAudit\' = TRUE /\\ UNCHANGED <<policy, notice, consumed>>',
        'Remember == phase = "Pending" /\\ phase\' = "Remembered" /\\ rule\' = TRUE /\\ ruleAudit\' = TRUE /\\ UNCHANGED <<policy, notice, consumed>>',
      ),
  },
  {
    document: "sdk-history-erasure",
    name: "Acknowledge erase reconciliation before the new incarnation exists",
    expectedRules: ["AcknowledgedNewIncarnation"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Reconcile == phase \\in {"ResetPending", "ResetObserved"} /\\ incarnation = 1 /\\ phase\' = "CleanupPending" /\\ resetAck\' = TRUE /\\ UNCHANGED <<incarnation, request, cleanup, access>>',
        'Reconcile == phase \\in {"ResetPending", "ResetObserved"} /\\ phase\' = "CleanupPending" /\\ resetAck\' = TRUE /\\ UNCHANGED <<incarnation, request, cleanup, access>>',
      ),
  },
  {
    document: "client-subscriptions",
    name: "Admit the next abandoned-target subscription before cleanup",
    expectedRules: ["AbortedOpenClosedBeforeNext"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'NextOpen == phase = "Closed" /\\ closeDone /\\ nextSent\' = TRUE /\\ UNCHANGED <<phase, aborted, answered, closeDone, buffer, done>>',
        "NextOpen == TRUE /\\ nextSent' = TRUE /\\ UNCHANGED <<phase, aborted, answered, closeDone, buffer, done>>",
      ),
  },
  {
    document: "client-event-registration",
    name: "Let an old unsubscribe remove a replacement registration",
    expectedRules: ["ReplacementSurvivesOldHandle"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "OldUnsubscribe == oldHandle /\\ current = 1 /\\ active' = FALSE /\\ oldHandle' = FALSE /\\ UNCHANGED <<current, reinstalled, staleRemoved>>",
        "OldUnsubscribe == oldHandle /\\ active' = FALSE /\\ oldHandle' = FALSE /\\ UNCHANGED <<current, reinstalled, staleRemoved>>",
      ),
  },
  {
    document: "sdk-context-admission",
    name: "Recompute the opening watermark after later admission settled",
    expectedRules: ["CapturedBoundaryPreserved"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "Publish == phase = \"Candidate\" /\\ phase' = \"Published\" /\\ boundary' = (IF sameId /\\ sameProvider THEN 0 ELSE captured) /\\ identityCommitted' = TRUE /\\ contextCommitted' = TRUE /\\ UNCHANGED <<count, prefix, captured, sameId, sameProvider, nativeMissing>>",
        "Publish == phase = \"Candidate\" /\\ phase' = \"Published\" /\\ boundary' = (IF sameId /\\ sameProvider THEN 0 ELSE prefix) /\\ identityCommitted' = TRUE /\\ contextCommitted' = TRUE /\\ UNCHANGED <<count, prefix, captured, sameId, sameProvider, nativeMissing>>",
      ),
  },
  {
    document: "sdk-context-admission",
    name: "Publish replacement identity without its context evidence",
    expectedRules: ["AtomicIdentityPublication"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        "Publish == phase = \"Candidate\" /\\ phase' = \"Published\" /\\ boundary' = (IF sameId /\\ sameProvider THEN 0 ELSE captured) /\\ identityCommitted' = TRUE /\\ contextCommitted' = TRUE /\\ UNCHANGED <<count, prefix, captured, sameId, sameProvider, nativeMissing>>",
        "Publish == phase = \"Candidate\" /\\ phase' = \"Published\" /\\ boundary' = (IF sameId /\\ sameProvider THEN 0 ELSE captured) /\\ identityCommitted' = TRUE /\\ contextCommitted' = FALSE /\\ UNCHANGED <<count, prefix, captured, sameId, sameProvider, nativeMissing>>",
      ),
  },
  {
    document: "sdk-context-validation",
    name: "Accept a watermark beyond the terminal invocation prefix",
    expectedRules: ["BoundaryWithinTerminal"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Validate == phase = "Inspecting" /\\ prefix <= count /\\ (IF absent THEN count = previous ELSE candidate <= prefix /\\ (~sameId \\/ candidate = previous \\/ namespace = 1)) /\\ phase\' = "Validated" /\\ valid\' = TRUE /\\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, previous>>',
        'Validate == phase = "Inspecting" /\\ prefix <= count /\\ (IF absent THEN count = previous ELSE TRUE /\\ (~sameId \\/ candidate = previous \\/ namespace = 1)) /\\ phase\' = "Validated" /\\ valid\' = TRUE /\\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, previous>>',
      ),
  },
  {
    document: "sdk-context-validation",
    name: "Reset a reused native ID without preceding net namespace evidence",
    expectedRules: ["SameIdResetNeedsPriorNetChange"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'Validate == phase = "Inspecting" /\\ prefix <= count /\\ (IF absent THEN count = previous ELSE candidate <= prefix /\\ (~sameId \\/ candidate = previous \\/ namespace = 1)) /\\ phase\' = "Validated" /\\ valid\' = TRUE /\\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, previous>>',
        'Validate == phase = "Inspecting" /\\ prefix <= count /\\ (IF absent THEN count = previous ELSE candidate <= prefix /\\ TRUE) /\\ phase\' = "Validated" /\\ valid\' = TRUE /\\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, previous>>',
      ),
  },
  {
    document: "sdk-stop-recovery",
    name: "Continue a captured final person or session Stop",
    expectedRules: ["CapturedStopNeverContinued"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'AdmitSuccessor == phase = "Settled" /\\ cause \\in {"Interrupted", "RetryableFailure"} /\\ ~declined /\\ edge /\\ phase\' = "Continued" /\\ successors\' = 1 /\\ UNCHANGED <<cause, edge, declined, restarts>>',
        'AdmitSuccessor == phase = "Settled" /\\ ~declined /\\ edge /\\ phase\' = "Continued" /\\ successors\' = 1 /\\ UNCHANGED <<cause, edge, declined, restarts>>',
      ),
  },
  {
    document: "sdk-stop-recovery",
    name: "Admit a successor after the persisted decline won",
    expectedRules: ["DeclineWinsBeforeAdmission"],
    mutate: (spec) =>
      replaceExactlyOnce(
        spec,
        'AdmitSuccessor == phase = "Settled" /\\ cause \\in {"Interrupted", "RetryableFailure"} /\\ ~declined /\\ edge /\\ phase\' = "Continued" /\\ successors\' = 1 /\\ UNCHANGED <<cause, edge, declined, restarts>>',
        'AdmitSuccessor == phase = "Settled" /\\ cause \\in {"Interrupted", "RetryableFailure"} /\\ edge /\\ phase\' = "Continued" /\\ successors\' = 1 /\\ UNCHANGED <<cause, edge, declined, restarts>>',
      ),
  },
]
