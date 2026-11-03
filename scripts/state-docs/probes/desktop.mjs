// Finite negative controls for desktop owners and the two local attachment UI owners.
// Each mutation removes one load-bearing identity, admission, publication or evidence rule.
// Expected invariant names come from pinned TLC counterexamples; the harness checks them again.
export const probes = [
  {
    document: "desktop-mcp-app-view",
    name: "AppMount safety weakening",
    expectedRules: ["InlineCannotTeardown"],
    mutate: (spec) => spec.replace('place # "inline"', "TRUE"),
  },
  {
    document: "desktop-create-channel",
    name: "ChannelOpening safety weakening",
    expectedRules: ["NavigationOwner"],
    mutate: (spec) =>
      spec.replace("IF owner = opening THEN owner ELSE navigated", "owner"),
  },
  {
    document: "desktop-workspace-fold-reveal-and-resize-side-columns",
    name: "ColumnChoice safety weakening",
    expectedRules: ["FitPreservesChoice"],
    mutate: (spec) => spec.replace("UNCHANGED wanted", "wanted' = TRUE"),
  },
  {
    document: "desktop-chat-compose-and-send-the-first-message",
    name: "ComposerObservation safety weakening",
    expectedRules: ["StaleCallbackPreservesReplacement"],
    mutate: (spec) => spec.replace("IF reply = observation THEN 0 ELSE observation", "0"),
  },
  {
    document: "desktop-workspace-open-dismiss-and-reopen-the-desktop-window",
    name: "DesktopWindowOrdering safety weakening",
    expectedRules: ["DockBeforeShow"],
    mutate: (spec) => spec.replace("dock' = TRUE", "dock' = FALSE"),
  },
  {
    document: "desktop-linked-devices",
    name: "DeviceApprovalDisplay safety weakening",
    expectedRules: ["ExactKeyApproval"],
    mutate: (spec) => spec.replace("fingerprint /\\ ~pending", "TRUE /\\ ~pending"),
  },
  {
    document: "desktop-devices-dashboard",
    name: "DevicesMount safety weakening",
    expectedRules: ["NoRetiredPublication"],
    mutate: (spec) => spec.replace("live /\\ pending", "TRUE /\\ pending"),
  },
  {
    document: "desktop-workspace-follow-focus-and-route-keyboard-actions",
    name: "FocusLanding safety weakening",
    expectedRules: ["TargetRechecked"],
    mutate: (spec) =>
      spec.replace(
        "captured = requested /\\ requested = marked",
        "TRUE /\\ requested = marked",
      ),
  },
  {
    document: "desktop-workspace-personalize-the-home-header-and-composer",
    name: "HeaderImageOwnership safety weakening",
    expectedRules: ["ChoiceOutranksInitialRead"],
    mutate: (spec) => spec.replace("IF touched THEN memory ELSE 0", "0"),
  },
  {
    document: "desktop-chat-rename-a-tab-and-inspect-conversation-details",
    name: "LocalTabTitle safety weakening",
    expectedRules: ["EditedTitleRetained"],
    mutate: (spec) =>
      spec.replace('IF tab \\in edited THEN @ ELSE "new-gateway"', '"new-gateway"'),
  },
  {
    document: "desktop-mcp-integration-management",
    name: "McpSettingsRequest safety weakening",
    expectedRules: ["NoFuturePublication"],
    mutate: (spec) => spec.replace("reply = pending", "TRUE"),
  },
  {
    document: "gateway-attachments-open-a-link-without-replacing-the-app",
    name: "NativeLinkHandoff safety weakening",
    expectedRules: ["OnlyWebToOpener"],
    mutate: (spec) => spec.replace('kind = "web" /\\ stage', "TRUE /\\ stage"),
  },
  {
    document: "desktop-workspace-open-split-resize-and-close-panes",
    name: "PaneAdmission safety weakening",
    expectedRules: ["UniqueItems"],
    mutate: (spec) =>
      spec.replace(
        "item \\notin Range(panes) /\\ Len(panes) < 2",
        "TRUE /\\ Len(panes) < 2",
      ),
  },
  {
    document: "desktop-workspace-resize-and-refit-the-panel-across-displays",
    name: "PanelSizePublication safety weakening",
    expectedRules: ["RetiredNoPublication"],
    mutate: (spec) =>
      spec.replace(
        "pending /\\ live /\\ pending' = FALSE",
        "pending /\\ TRUE /\\ pending' = FALSE",
      ),
  },
  {
    document: "desktop-workspace-summon-or-dismiss-the-floating-panel",
    name: "PanelToggle safety weakening",
    expectedRules: ["OnePressedEdge"],
    mutate: (spec) =>
      spec
        .replace(
          "UNCHANGED <<shown,pressed,toggles,reported>>",
          "toggles' = toggles + 1 /\\ UNCHANGED <<shown,pressed,reported>>",
        )
        .replace("Repeat == /\\ pressed", "Repeat == /\\ pressed /\\ toggles < 2"),
  },
  {
    document: "desktop-provider-sign-in",
    name: "ProviderRecovery safety weakening",
    expectedRules: ["RecoveryCausality"],
    mutate: (spec) => spec.replace("published \\notin observed", "TRUE"),
  },
  {
    document: "desktop-question-flights",
    name: "QuestionFlights safety weakening",
    expectedRules: ["NoConflictingJoin"],
    mutate: (spec) => spec.replace("stamp = slot /\\ joined", "TRUE /\\ joined"),
  },
  {
    document: "gateway-attachments-read-rich-text-code-math-and-diagrams-in-a-message",
    name: "RichTextRendering safety weakening",
    expectedRules: ["PublicationIdentity"],
    mutate: (spec) =>
      spec.replace(
        "published' = (IF Wanted(active) THEN active ELSE 0)",
        "published' = active",
      ),
  },
  {
    document: "desktop-permission-rule-revocation",
    name: "RuleRevocation safety weakening",
    expectedRules: ["RereadMeaning"],
    mutate: (spec) => spec.replace('IF actual THEN "retained" ELSE "absent"', '"absent"'),
  },
  {
    document: "desktop-settings-action-owners",
    name: "SettingsActionOwners safety weakening",
    expectedRules: ["GrantReplay"],
    mutate: (spec) =>
      spec.replace(
        'noticeOwner = binding /\\ notice\' = "Granted"',
        'TRUE /\\ notice\' = "Granted"',
      ),
  },
  {
    document: "desktop-conversation-sharing",
    name: "SharingDialog safety weakening",
    expectedRules: ["PendingHoldsForm"],
    mutate: (spec) => spec.replace("~changing /\\ needsRead", "TRUE /\\ needsRead"),
  },
  {
    document: "desktop-window-preferences",
    name: "StoredPreference safety weakening",
    expectedRules: ["LocalWriteFailureStillApplies"],
    mutate: (spec) => spec.replace("local' = value", "local' = stored"),
  },
  {
    document: "desktop-chat-recover-a-refused-or-unconfirmed-send",
    name: "SubmissionRecovery safety weakening",
    expectedRules: ["StableRetry"],
    mutate: (spec) =>
      spec.replace("retryIdentity' = identity", 'retryIdentity\' = "input-B"'),
  },
  {
    document: "desktop-workspace-switch-the-floating-panel-s-surface",
    name: "SurfaceSubscription safety weakening",
    expectedRules: ["NoRetiredInvocation"],
    mutate: (spec) =>
      spec.replace(
        "event' = \"late\" /\\ invoked' = FALSE",
        "event' = \"late\" /\\ invoked' = TRUE",
      ),
  },
  {
    document: "desktop-chat-close-switch-reopen-and-restore-tabs",
    name: "TabLifetimes safety weakening",
    expectedRules: ["FreshLocalLifetime"],
    mutate: (spec) => spec.replace("live' = {counter + 1}", "live' = live"),
  },
  {
    document: "desktop-voice-call",
    name: "VoiceGeneration safety weakening",
    expectedRules: ["CurrentConnection"],
    mutate: (spec) =>
      spec.replace("IF call = current THEN {call} ELSE connected", "{call}"),
  },
  {
    document: "desktop-workspace-open-resize-expand-and-restore-the-right-panel",
    name: "WidgetPanel safety weakening",
    expectedRules: ["PerChatOwnership"],
    mutate: (spec) => spec.replace("tabs' = byChat[next]", "tabs' = tabs"),
  },
  {
    document: "desktop-workspace-load-browse-and-resynchronize-the-workspace",
    name: "WorkspaceRevision safety weakening",
    expectedRules: ["RemovalFence"],
    mutate: (spec) => spec.replace("revision > removed", "TRUE"),
  },
]
