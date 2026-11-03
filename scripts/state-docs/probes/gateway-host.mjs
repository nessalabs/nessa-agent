// Finite safety weakenings for the gateway, native host and relay owners.
// The harness resolves each document through the catalog and never edits its model.
export const probes = [
  {
    document: "host-startup-save-provider-credentials-and-download-a-runtime",
    name: "Write a provider key without acknowledged intent",
    expectedRules: ["IntentBeforeEffect"],
    mutate: (spec) =>
      spec
        .replaceAll("intent' = (phase' = \"Write\")", "intent' = FALSE")
        .replaceAll('/\\ intent /\\ phase\' = "Outcome"', '/\\ phase\' = "Outcome"'),
  },
  {
    document: "gateway-background-tasks",
    name: "Finish continuation delivery before a task outcome exists",
    expectedRules: ["OutcomeBeforeDelivery"],
    mutate: (spec) =>
      spec.replaceAll('outcome # "None" /\\ delivery', "TRUE /\\ delivery"),
  },
  {
    document: "gateway-attachments-upload-an-image-and-retry-a-failed-upload",
    name: "Retarget an upload when the active tab changes",
    expectedRules: ["CapturedTarget"],
    mutate: (spec) =>
      spec.replaceAll(
        "UNCHANGED <<phase,present,target,staged>>",
        "target' = activeTab /\\ UNCHANGED <<phase,present,staged>>",
      ),
  },
  {
    document: "runtime-download",
    name: "Keep stale runtime offers after an uncertain installation",
    expectedRules: ["FreshOffers"],
    mutate: (spec) =>
      spec.replaceAll(
        "offers' = FALSE /\\ floor' = read",
        "offers' = TRUE /\\ floor' = read",
      ),
  },
  {
    document: "github-webhooks",
    name: "Read an inbox after a denied policy decision",
    expectedRules: ["PrivateRead"],
    mutate: (spec) => spec.replaceAll("/\\ audited /\\ allowed /\\", "/\\ audited /\\"),
  },
  {
    document: "gateway-webhook-foundation",
    name: "Allow a sixth delivery attempt",
    expectedRules: ["AttemptBound"],
    mutate: (spec) => spec.replaceAll("< 5", "< 6"),
  },
  {
    document: "gateway-webhook-admission",
    name: "Acquire a grant without its credential fence",
    expectedRules: ["PartialOrder", "CheckedOwnsFences"],
    mutate: (spec) => spec.replaceAll("credential' = TRUE", "credential' = FALSE"),
  },
  {
    document: "gateway-webhook-issuance",
    name: "Issue a credential before reserving its intent",
    expectedRules: ["ReservationBeforeIssue"],
    mutate: (spec) =>
      spec
        .replaceAll("intent' = TRUE", "intent' = FALSE")
        .replaceAll("/\\ intent /\\ issued", "/\\ issued"),
  },
  {
    document: "gateway-webhook-outbox",
    name: "POST without durable Sending ownership",
    expectedRules: ["PersistBeforePost"],
    mutate: (spec) => spec.replaceAll("sendingSaved' = TRUE", "sendingSaved' = FALSE"),
  },
  {
    document: "gateway-record-reads",
    name: "Release capacity while a timed out source is still running",
    expectedRules: ["RetainedUntilJoin"],
    mutate: (spec) =>
      spec.replaceAll(
        "caller' = FALSE /\\ UNCHANGED <<lease,joined>>",
        "caller' = FALSE /\\ lease' = \"None\" /\\ UNCHANGED joined",
      ),
  },
  {
    document: "gateway-channel-workspaces",
    name: "Grant workspace authority without verification",
    expectedRules: ["VerifyBeforeAuthority"],
    mutate: (spec) =>
      spec.replaceAll("verified' = (phase' = \"Ready\")", "verified' = FALSE"),
  },
  {
    document: "gateway-github-reads",
    name: "Return an answer on a refused read",
    expectedRules: ["RefusalHasNoAnswer"],
    mutate: (spec) =>
      spec.replaceAll(
        'phase\' = "Refused" /\\ UNCHANGED <<generation,captured,stopped,answered>>',
        "phase' = \"Refused\" /\\ answered' = TRUE /\\ UNCHANGED <<generation,captured,stopped>>",
      ),
  },
  {
    document: "local-vault",
    name: "Publish an unlocked key after its owner closes",
    expectedRules: ["LiveOwner"],
    mutate: (spec) =>
      spec.replaceAll("/\\ key /\\ ownerLive /\\ generation", "/\\ key /\\ generation"),
  },
  {
    document: "gateway-attachments-attach-files-with-the-picker-or-clipboard",
    name: "Redeem one native ticket twice",
    expectedRules: ["SingleUse"],
    mutate: (spec) => spec.replaceAll("reads' = reads + 1", "reads' = 2"),
  },
  {
    document: "surface-session-supervision",
    name: "Keep a usable handle while reconnecting",
    expectedRules: ["PublishedOnlyOwned"],
    mutate: (spec) =>
      spec.replaceAll(
        "phase' = \"Reconnecting\" /\\ handle' = FALSE",
        "phase' = \"Reconnecting\" /\\ handle' = TRUE",
      ),
  },
  {
    document: "gateway-startup-observation",
    name: "Publish host state without delivered observation evidence",
    expectedRules: ["HostHasEvidence"],
    mutate: (spec) => spec.replaceAll("delivered' = TRUE", "delivered' = FALSE"),
  },
  {
    document: "host-startup-complete-setup-choose-an-agent-and-learn-the-shortcut",
    name: "Select an agent without a ready report",
    expectedRules: ["PickerHasReadyChoice"],
    mutate: (spec) =>
      spec.replaceAll(
        'phase = "Agent" /\\ ready /\\ chosen',
        'phase = "Agent" /\\ chosen',
      ),
  },
  {
    document:
      "gateway-attachments-have-a-large-or-unfamiliar-image-normalized-and-kept-once",
    name: "Confirm a hold without acknowledged creation audit",
    expectedRules: ["AuditBeforeConfirm"],
    mutate: (spec) =>
      spec.replaceAll("audited' = (phase' = \"Audited\")", "audited' = FALSE"),
  },
  {
    document:
      "gateway-attachments-wait-for-a-cloud-file-without-attaching-it-to-another-tab",
    name: "Attach a completed read to the currently selected tab",
    expectedRules: ["OriginalConversation"],
    mutate: (spec) => spec.replaceAll("THEN captured ELSE 0", "THEN active ELSE 0"),
  },
  {
    document: "host-startup-sign-in-restore-renew-and-sign-out-in-the-browser",
    name: "Keep local access while server logout is pending",
    expectedRules: ["LocalTeardownFirst"],
    mutate: (spec) => spec.replaceAll("local' = FALSE", "local' = TRUE"),
  },
  {
    document: "host-startup-launch-and-choose-the-first-surface",
    name: "Start a gateway without required host composition",
    expectedRules: ["RequiredBeforeGateway"],
    mutate: (spec) =>
      spec
        .replaceAll("required' = (phase' = \"Ready\")", "required' = FALSE")
        .replaceAll("/\\ required /\\ gateway", "/\\ gateway"),
  },
  {
    document: "gateway-conversation",
    name: "Erase local history before deletion audit acknowledges",
    expectedRules: ["ErasurePrerequisites"],
    mutate: (spec) =>
      spec
        .replaceAll("audit' = TRUE", "audit' = FALSE")
        .replaceAll("/\\ audit /\\ erased", "/\\ erased"),
  },
  {
    document: "gateway-external-events",
    name: "Dispatch the same event identity twice",
    expectedRules: ["OneEventIdentity", "AckIsDispatch"],
    mutate: (spec) => spec.replaceAll("dispatches' = 1", "dispatches' = 2"),
  },
  {
    document: "github-relay-live-selection",
    name: "Remove selected events without deletion evidence",
    expectedRules: ["DeletionEvidence"],
    mutate: (spec) => spec.replaceAll("audit' = TRUE", "audit' = FALSE"),
  },
  {
    document: "gateway-attachments-send-a-non-image-file-by-path-and-approve-its-read",
    name: "Dispatch a named path before durable naming evidence",
    expectedRules: ["EvidenceBeforeDispatch"],
    mutate: (spec) =>
      spec
        .replaceAll("saved' = (phase' = \"Admit\")", "saved' = FALSE")
        .replaceAll("/\\ saved /\\ dispatched", "/\\ dispatched"),
  },
  {
    document: "gateway-attachments-send-an-uploaded-file-from-the-phone",
    name: "Place an uploaded file before recording Asked evidence",
    expectedRules: ["AskBeforeEffect"],
    mutate: (spec) =>
      spec
        .replaceAll("asked' = TRUE", "asked' = FALSE")
        .replaceAll('/\\ asked /\\ phase\' = "Observed"', '/\\ phase\' = "Observed"'),
  },
  {
    document: "gateway-attachments-preview-or-remove-a-draft-attachment",
    name: "Release local bytes while draft or preview still needs them",
    expectedRules: ["RetainWhileNeeded"],
    mutate: (spec) => spec.replaceAll("~draft /\\ ~preview /\\ retained", "retained"),
  },
  {
    document: "host-startup-recover-or-update-the-managed-gateway",
    name: "Start deferred copying without a durable plan",
    expectedRules: ["PlanBeforeCopy"],
    mutate: (spec) =>
      spec
        .replaceAll("plan' = (phase' = \"Accepted\")", "plan' = FALSE")
        .replaceAll("/\\ plan /\\ ready", "/\\ ready"),
  },
  {
    document: "host-startup-bootstrap-and-diagnose-access-through-the-cli",
    name: "Choose the wider requested or caller credential lifetime",
    expectedRules: ["LifetimeCapped"],
    mutate: (spec) =>
      spec.replaceAll(
        "IF requested < callerExpiry THEN requested ELSE callerExpiry",
        "IF requested > callerExpiry THEN requested ELSE callerExpiry",
      ),
  },
  {
    document: "gateway-device-pairing",
    name: "remove canonical current holding guard",
    expectedRules: ["ActiveNeedsCurrentReceiver"],
    mutate: (spec) => spec.replace('/\\ current = "Held" /\\ proof', "/\\ proof"),
  },
  {
    document: "gateway-device-pairing",
    name: "remove fresh publication admission",
    expectedRules: ["FreshPublication"],
    mutate: (spec) => spec.replace("/\\ admission = revision /\\ proof", "/\\ proof"),
  },
  {
    document: "gateway-device-pairing",
    name: "remove exact approved key guard",
    expectedRules: ["ExactApprovedKey"],
    mutate: (spec) => spec.replace("/\\ key = claimed /\\ approved", "/\\ approved"),
  },
  {
    document: "gateway-parent-stop",
    name: "remove root digest restoration guard",
    expectedRules: ["ExactCapturedEffects"],
    mutate: (spec) =>
      spec.replace(
        "/\\ rootDigest = ({1} \\cup children) /\\ validated",
        "/\\ validated",
      ),
  },
  {
    document: "gateway-parent-stop",
    name: "decline attempted without confirmed receipt",
    expectedRules: ["ReceiptBeforeDecline"],
    mutate: (spec) =>
      spec.replace(
        "target \\in confirmed \\ declined",
        "target \\in attempted \\ declined",
      ),
  },
  {
    document:
      "gateway-attachments-paste-text-or-code-and-inspect-the-full-pasted-payload",
    name: "retarget viewer to another tab",
    expectedRules: ["ViewerRetainsPayload"],
    mutate: (spec) => spec.replace("owner' = active", "owner' = 3 - active"),
  },
  {
    document:
      "gateway-attachments-understand-an-attachment-refusal-and-choose-a-useful-recovery",
    name: "dismiss attempt clears independent draft fact",
    expectedRules: ["DismissDoesNotResolveFact"],
    mutate: (spec) =>
      spec
        .replace(
          "notice' = [notice EXCEPT ![target] = FALSE] /\\ answered'",
          "fact' = FALSE /\\ notice' = [notice EXCEPT ![target] = FALSE] /\\ answered'",
        )
        .replace(
          "UNCHANGED <<active,scope,fact,resolved>>\nRestoreDraft",
          "UNCHANGED <<active,scope,resolved>>\nRestoreDraft",
        ),
  },
  {
    document: "gateway-drive-tool-admission",
    name: "Steer after saving without rechecking the current grant revision",
    expectedRules: ["NativeSteerRechecksRevision"],
    mutate: (spec) =>
      spec.replace('phase = "Recheck" /\\ current = admitted', 'phase = "Recheck"'),
  },
  {
    document: "gateway-drive-tool-admission",
    name: "Run a granted tool without acknowledged decision audit",
    expectedRules: ["ToolEffectHasDecisionAudit"],
    mutate: (spec) =>
      spec
        .replace("audited' = TRUE", "audited' = FALSE")
        .replace(
          'decision = "Allow" /\\ audited /\\ effect',
          'decision = "Allow" /\\ effect',
        ),
  },
  {
    document: "gateway-environment-tools",
    name: "Accept wider timeout or foreign sandbox from the remote command grant",
    expectedRules: ["GrantedTermsDoNotWiden"],
    mutate: (spec) =>
      spec.replace('/\\ granted <= asked /\\ sandbox = "None" /\\ hold', "/\\ hold"),
  },
  {
    document: "gateway-environment-tools",
    name: "Invoke a granted command before saving CommandIssued",
    expectedRules: ["SaveIssueBeforeRun"],
    mutate: (spec) =>
      spec.replace(
        'Run == /\\ phase = "Issued" /\\ issued /\\ hold',
        'Run == /\\ phase = "Granted" /\\ hold',
      ),
  },
  {
    document: "gateway-drive-tool-admission",
    name: "Save a Drive turn under a replaced grant revision",
    expectedRules: ["SaveUsesHeldRevision"],
    mutate: (spec) =>
      spec.replace("retained /\\ current = admitted /\\ held", "retained /\\ held"),
  },
  {
    document: "gateway-environment-tools",
    name: "Keep and record unverified artifact bytes",
    expectedRules: ["ArtifactRecordHasVerifiedHold"],
    mutate: (spec) =>
      spec.replace(
        'artifact = "None" /\\ valid /\\ artifact',
        'artifact = "None" /\\ artifact',
      ),
  },
  {
    document: "gateway-record-read-discovery",
    name: "Charge an occupied cache checkout as a validating discovery step",
    expectedRules: ["OccupiedDoesNotSpendSteps"],
    mutate: (spec) =>
      spec.replace(
        'phase\' = "Waiting" /\\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>',
        "phase' = \"Waiting\" /\\ steps' = steps + 1 /\\ UNCHANGED <<started,elapsed,validated,targetOccupied,otherOccupied,held>>",
      ),
  },
  {
    document: "gateway-record-read-discovery",
    name: "Expire the discovery work budget before its worker starts",
    expectedRules: ["QueueDoesNotSpendWorkBudget"],
    mutate: (spec) =>
      spec.replace(
        'Elapse == /\\ started /\\ ~elapsed /\\ phase \\in {"Discovering","Waiting"}',
        "Elapse == /\\ ~elapsed",
      ),
  },
  {
    document: "gateway-record-read-discovery",
    name: "Wait for an unrelated occupied stream before retrying this free stream",
    expectedRules: ["OwnStreamCanResume"],
    mutate: (spec) =>
      spec.replace(
        'Resume == /\\ phase = "Waiting" /\\ ~targetOccupied /\\ ~elapsed',
        'Resume == /\\ phase = "Waiting" /\\ ~targetOccupied /\\ ~otherOccupied /\\ ~elapsed',
      ),
  },
  {
    document: "gateway-record-read-discovery",
    name: "Release source capacity while waiting on occupied discovery",
    expectedRules: ["CapacityUntilSourceJoin"],
    mutate: (spec) =>
      spec.replace(
        'phase\' = "Waiting" /\\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied,held>>',
        "phase' = \"Waiting\" /\\ held' = FALSE /\\ UNCHANGED <<started,elapsed,steps,validated,targetOccupied,otherOccupied>>",
      ),
  },
]
