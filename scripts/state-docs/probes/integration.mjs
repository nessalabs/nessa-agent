// Independent order, correlation and evidence controls for the integration owners.
export const probes = [
  {
    document: "mcp-resource-tickets",
    name: "Redeem a consumed ticket a second time",
    expectedRules: ["AtMostOnce"],
    mutate: (s) =>
      s.replace(
        'phase = "Active" /\\ held',
        'phase \\in {"Active", "Redeemed"} /\\ redemptions < 2',
      ),
  },
  {
    document: "sdk-cleanup",
    name: "Reopen after a failed cleanup audit",
    expectedRules: ["ReopenRequiresAudit"],
    mutate: (s) =>
      s.replace(
        '/\\ audit = "Acknowledged"\n          /\\ retired',
        '/\\ audit # "Pending"\n          /\\ retired',
      ),
  },
  {
    document: "gateway-backup-export",
    name: "Acknowledge an export before its cut is complete",
    expectedRules: ["AckRequiresCompleteCut"],
    mutate: (s) =>
      s.replace(
        'phase = "Authenticated" /\\ pinned /\\ completeOutboxes /\\ mac /\\ ~held',
        'phase = "Idle"',
      ),
  },
  {
    document: "gateway-backup-restore",
    name: "Accept an older retained receipt generation",
    expectedRules: ["AcceptanceHasCurrentProof"],
    mutate: (s) => s.replace("candidate[1] = generation", "TRUE"),
  },
  {
    document: "gateway-backup-restore",
    name: "Accept a changed journal under an older digest",
    expectedRules: ["AcceptanceHasCurrentProof"],
    mutate: (s) =>
      s
        .replaceAll("candidate[2] = actualDigest", "TRUE")
        .replaceAll("sealedDigest = actualDigest", "TRUE"),
  },
  {
    document: "gateway-backup-restore",
    name: "Accept a previously spent stage",
    expectedRules: ["AcceptanceHasCurrentProof"],
    mutate: (s) => s.replaceAll("generation \\notin spent", "TRUE"),
  },
  {
    document: "gateway-agent-thread-tools",
    name: "Mint source capability after a denied policy result",
    expectedRules: ["EffectHasAuthority"],
    mutate: (s) =>
      s.replace(
        "slot /\\ verified /\\ allowed /\\ audit /\\ ~stopped",
        "slot /\\ verified /\\ audit /\\ ~stopped",
      ),
  },
  {
    document: "gateway-agent-thread-tools",
    name: "Keep source admission charged through target dispatch",
    expectedRules: ["SourcePermitEndsAtAuthorization"],
    mutate: (s) =>
      s.replace(
        "phase' = \"Authorized\" /\\ slot' = FALSE",
        "phase' = \"Authorized\" /\\ slot' = TRUE",
      ),
  },
  {
    document: "gateway-peer-gateways",
    name: "Interrupt a peer read before command intent audit",
    expectedRules: ["PreemptionNeedsIntent"],
    mutate: (s) =>
      s.replace('phase = "SetHeld" /\\ setHeld /\\ intent', 'phase = "Idle"'),
  },
  {
    document: "gateway-environment-leases",
    name: "Use wider grant timeout instead of narrowing to the request",
    expectedRules: ["TermsNeverWiden"],
    mutate: (s) =>
      s.replace(
        "effectiveTimeout' = (IF askedTimeout <= grantTimeout THEN askedTimeout ELSE grantTimeout)",
        "effectiveTimeout' = grantTimeout",
      ),
  },
  {
    document: "gateway-tunnel-connectivity",
    name: "Dispatch based on tunnel reachability without native admission",
    expectedRules: ["ForwardingIsNotAuthority"],
    mutate: (s) => s.replace("nativeAdmitted /\\ ~dispatched", "~dispatched"),
  },
]
