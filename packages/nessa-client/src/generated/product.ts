/* eslint-disable */
/* Generated from protocol/product/v1.json and manifest.json. Do not edit. */
/** Server challenge received before authentication. The SDK checks version overlap, then returns the nonce with credential evidence; callers normally use NessaClient.connect instead of building this payload. */
export interface SessionChallenge {
  /** Lowest protocol version accepted by the gateway. */
  minVersion: number
  /** Highest protocol version accepted by the gateway. */
  maxVersion: number
  /** Single-handshake challenge value to return unchanged when authenticating. */
  nonce: string
  /** Challenge deadline as Unix seconds, rounded up from millisecond wall time. The server enforces one monotonic timeout including challenge delivery; expiry closes with retryable handshake_timeout. */
  expiresAt: number
}
/** Caller identification sent during the product handshake. This metadata does not grant permissions. */
export interface ProductClientMetadata {
  /** Identifier of the connecting client application or instance. */
  id: string
}
/** Kind of user-facing surface. The same closed set as protocol/schemas/v1/common.json SurfaceKind; generation refuses a drift. Metadata only: it does not grant permissions. */
export const ProductSurfaceKind = {
  Panel: "panel",
  Web: "web",
  Desktop: "desktop",
  Cli: "cli",
} as const
export type ProductSurfaceKind =
  (typeof ProductSurfaceKind)[keyof typeof ProductSurfaceKind]
/** Surface the handshake names so the gateway can tell connections that share a credential apart. The credential still decides identity. */
export interface ProductSurface {
  /** Kind of user-facing surface. */
  kind: ProductSurfaceKind
  /** Identifier distinguishing instances of this surface. */
  instance: string
}
/** Wire request for session.authenticate. NessaClient.connect builds this after validating the server challenge; a successful response is ProductSessionReady. */
export interface SessionAuthenticateParams {
  /** Lowest protocol version supported by this client. */
  minVersion: number
  /** Highest protocol version supported by this client. */
  maxVersion: number
  /** Nonce from the current SessionChallenge; a reconnect obtains a new one. */
  nonce: string
  /** Opaque secret credential evidence. Send only during authentication; do not log it. */
  credential: string
  /** Descriptive client metadata, separate from authenticated identity. */
  client: ProductClientMetadata
  /** Surface kind and instance. Descriptive only; the credential decides the principal. */
  surface: ProductSurface
}
/** Authenticated connection snapshot returned by the product handshake and auth.session(). Links the principal, organization membership, credential, and gateway. It describes current restrictions; the server checks current authorization again for every command. */
export interface ProductSessionReady {
  /** Negotiated product protocol version. */
  version: 1
  /** Identifier of the connected gateway. */
  gatewayId: string
  /** Authenticated human, integration, agent, or paired peer gateway identity. */
  principalId: string
  /** Organization in which this session operates. */
  organizationId: string
  /** Membership connecting the principal to this organization. */
  membershipId: string
  /** Credential used to authenticate this connection. */
  credentialId: string
  /** Gateway audience to which the credential is bound. */
  audienceId: string
  /** Exclusive Unix seconds, or null for no expiry. */
  expiresAt: number | null
  /** Current credential restrictions; not permanent authorization. */
  grants: ProductGrant[]
  /** Registered product methods; permission is checked for every command. */
  methods: string[]
}
/** Identity that acts in the product: a human, integration, agent, or paired peer gateway. A principal needs an active organization membership and an appropriate credential to access the gateway. */
export interface ProductPrincipal {
  /** Stable identifier for this identity. */
  id: string
  /** Category of actor; this alone does not grant access. */
  kind: PrincipalKind
}
/** Relationship between a principal and an organization. The role participates in authorization and disabled membership prevents access. */
export interface ProductMembership {
  /** Stable identifier of this membership. */
  id: string
  /** Principal associated with the membership. */
  principalId: string
  /** Organization the principal belongs to. */
  organizationId: string
  /** Organization role evaluated by authorization policy. */
  role: MembershipRole
  /** Whether the membership is active or disabled. */
  state: MembershipState
}
/** Organization-scoped authorization target used by ProductGrant. The gateway resolves the actual resource for each operation before checking policy. */
export interface ProductResource {
  /** Organization owning the resource; must match the credential organization. */
  organizationId: string
  /** Resource identifier, such as the gateway ID for server operations. */
  id: string
}
/** One action/resource restriction on a credential. Used in credential issuance, credential metadata, and authenticated session snapshots. A grant is not a standalone permission: current membership and server policy must also allow the operation.

For example, { action: "server.read", resource: { organizationId: "personal", id: "gateway" } } scopes server-read access to that gateway; use the actual organization and gateway identifiers from your installation. */
export interface ProductGrant {
  /** Authorization action, for example server.read. This is a policy action rather than an RPC method name such as server.health. */
  action: string
  /** Exact organization-scoped target on which this action is requested. */
  resource: ProductResource
}
/** Non-secret description of a credential returned by issue and list operations. Describes who it authenticates, its gateway audience, lifetime, and grant restrictions; it cannot itself authenticate a connection. */
export interface ProductCredentialMetadata {
  /** Identifier used to revoke this credential. */
  id: string
  /** Identity authenticated by the credential. */
  principalId: string
  /** Organization in which the credential applies. */
  organizationId: string
  /** Gateway audience allowed to accept this credential. */
  audienceId: string
  /** Creation time in Unix seconds. */
  issuedAt: number
  /** Exclusive Unix seconds, or null for no expiry. */
  expiresAt: number | null
  /** Revocation time in Unix seconds, or null if not revoked. */
  revokedAt: number | null
  /** Action/resource restrictions evaluated along with current server policy. */
  grants: ProductGrant[]
}
/** Wire input for issuing a scoped credential. Prefer client.credentials.issue, whose IssueCredentialParams allows the SDK to generate requestId. Reuse the same ID and inputs when explicitly retrying an uncertain result. */
export interface CredentialIssueParams {
  /** Idempotency key identifying this issuance operation. */
  requestId: string
  /** Identity the new credential will authenticate. */
  principal: ProductPrincipal
  /** Organization membership associated with that principal. */
  membership: ProductMembership
  /** Exclusive Unix seconds, or null for no expiry. */
  expiresAt?: number | null
  /** Requested action/resource restrictions; at least one is required. */
  grants: ProductGrant[]
}
/** Empty wire input for credential.list. The SDK list() method supplies this object. */
export interface CredentialListParams {}
/** Wire input for credential.revoke. Prefer client.credentials.revoke to generate and preserve the request ID. */
export interface CredentialRevokeParams {
  /** Idempotency key to reuse for retries of this revocation. */
  requestId: string
  /** Identifier of the credential to revoke. */
  credentialId: string
}
/** First successful issuance response. Contains secret evidence only once; keep it in private storage before discarding the result. */
export interface IssuedCredentialResult {
  /** Non-secret metadata for the issued credential. */
  credential: ProductCredentialMetadata
  /** One-time credential evidence accepted by product connect auth. It is not replayed on a duplicate request. */
  secret: string
}
/** Response when the issuance request ID already completed. Metadata is returned, but the original secret cannot be recovered by retrying. */
export interface ExistingCredentialResult {
  /** Metadata for the previously issued credential. */
  credential: ProductCredentialMetadata
  /** Always true: this response does not contain a usable secret. */
  secretUnavailable: true
}
/** Credentials visible to the authorized caller, without secret evidence. */
export interface CredentialListResult {
  /** Non-secret credential records returned by the gateway. */
  credentials: ProductCredentialMetadata[]
}
/** Acknowledgement of a credential revocation. */
export interface CredentialRevokeResult {
  /** Identifier of the revoked credential. */
  credentialId: string
  /** Authorization-state revision returned by the mutation. */
  revision: number
}
/** Product session termination reasons used to determine whether connection recovery is allowed. Revocation, expiry, authorization loss, and incompatible protocols are terminal; transient transport and gateway failures may retry. */
export const SessionCloseReason = {
  AuthenticationFailed: "authentication_failed",
  CredentialRevoked: "credential_revoked",
  CredentialExpired: "credential_expired",
  AuthorizationLost: "authorization_lost",
  ProtocolIncompatible: "protocol_incompatible",
  HandshakeTimeout: "handshake_timeout",
  TemporaryUnavailable: "temporary_unavailable",
  GatewayRestarting: "gateway_restarting",
  GatewayOverloaded: "gateway_overloaded",
  ServerShutdown: "server_shutdown",
  TransportInterrupted: "transport_interrupted",
} as const
export type SessionCloseReason =
  (typeof SessionCloseReason)[keyof typeof SessionCloseReason]
export const sessionClosePolicy = {
  authentication_failed: { webSocketCode: 4001, retryable: false },
  credential_revoked: { webSocketCode: 4002, retryable: false },
  credential_expired: { webSocketCode: 4003, retryable: false },
  authorization_lost: { webSocketCode: 4004, retryable: false },
  protocol_incompatible: { webSocketCode: 4005, retryable: false },
  handshake_timeout: { webSocketCode: 4006, retryable: true },
  temporary_unavailable: { webSocketCode: 4010, retryable: true },
  gateway_restarting: { webSocketCode: 1012, retryable: true },
  gateway_overloaded: { webSocketCode: 1013, retryable: true },
  server_shutdown: { webSocketCode: 1000, retryable: false },
  transport_interrupted: { webSocketCode: 1006, retryable: true },
} as const
/** Supported categories of authenticated actors: human users, integrations, agents, and paired peer gateways. Only device pairing creates a gateway principal; credential.issue refuses one. */
export const PrincipalKind = {
  Human: "human",
  Integration: "integration",
  Agent: "agent",
  Gateway: "gateway",
} as const
export type PrincipalKind = (typeof PrincipalKind)[keyof typeof PrincipalKind]
/** Organization roles understood by product authorization policy. */
export const MembershipRole = { Admin: "admin", Member: "member" } as const
export type MembershipRole = (typeof MembershipRole)[keyof typeof MembershipRole]
/** Membership availability. Disabled membership prevents access even when credential evidence is valid. */
export const MembershipState = { Active: "active", Disabled: "disabled" } as const
export type MembershipState = (typeof MembershipState)[keyof typeof MembershipState]
/** Structured WebSocket close reason. The SDK derives retry policy from the numeric close code and accepts matching, bounded server delay hints. */
export interface SessionTermination {
  /** Product reason corresponding to the WebSocket close code. */
  code: SessionCloseReason
  /** Whether the reason permits connection recovery; cannot override numeric-code policy. */
  retryable: boolean
  /** Optional server delay hint in milliseconds, bounded to 60,000. */
  retryAfterMs?: number
}
/** Current invocation status, including unresolved interrupted history. */
export const ConversationMessageStatus = {
  Queued: "queued",
  Running: "running",
  Completed: "completed",
  Cancelled: "cancelled",
  Failed: "failed",
  Injected: "injected",
  Unresolved: "unresolved",
} as const
export type ConversationMessageStatus =
  (typeof ConversationMessageStatus)[keyof typeof ConversationMessageStatus]
/** How an admitted input waits for dispatch. */
export const ConversationPendingMode = { Queued: "queued", Steering: "steering" } as const
export type ConversationPendingMode =
  (typeof ConversationPendingMode)[keyof typeof ConversationPendingMode]
/** How the gateway accepted or recovered this submission. */
export const ConversationDisposition = {
  Queued: "queued",
  Injected: "injected",
  Settled: "settled",
} as const
export type ConversationDisposition =
  (typeof ConversationDisposition)[keyof typeof ConversationDisposition]
/** Current provider attachment state. This is presentation state and grants no operation authority. */
export const ConversationLifecyclePhase = {
  Absent: "absent",
  Starting: "starting",
  Attached: "attached",
  Failed: "failed",
} as const
export type ConversationLifecyclePhase =
  (typeof ConversationLifecyclePhase)[keyof typeof ConversationLifecyclePhase]
/** Stable category for a provider attachment failure. */
export const ConversationStartupFailureCode = {
  Audit: "audit",
  Provider: "provider",
  Storage: "storage",
  Cleanup: "cleanup",
} as const
export type ConversationStartupFailureCode =
  (typeof ConversationStartupFailureCode)[keyof typeof ConversationStartupFailureCode]
/** Bounded presentation of the latest provider attachment failure. It carries no admission or cleanup authority. */
export interface ConversationStartupFailure {
  /** Stable failure category. */
  code: ConversationStartupFailureCode
  /** Bounded diagnostic suitable for display, at most 2048 UTF-8 bytes. `x-utf8MaxBytes` is the authoritative byte bound; `maxLength` is a coarse code-point bound. */
  message: string
}
/** Bounded late failure to acknowledge mandatory attachment audit evidence. It carries no lifecycle authority. */
export interface ConversationAttachmentEvidenceFailure {
  /** Mandatory attachment audit was not acknowledged. */
  code: "audit"
  /** Bounded diagnostic suitable for display, at most 2048 UTF-8 bytes. `x-utf8MaxBytes` is the authoritative byte bound; `maxLength` is a coarse code-point bound. */
  message: string
}
/** Current provider attachment lifecycle for this conversation. */
export interface ConversationLifecycle {
  /** Current provider attachment phase. */
  phase: ConversationLifecyclePhase
  /** Present exactly when phase is failed. */
  failure?: ConversationStartupFailure
  /** Late mandatory attachment-audit failure retained independently of the current phase. It grants no lifecycle authority. */
  evidenceFailure?: ConversationAttachmentEvidenceFailure
}
/** Whether Nessa can deliver a deny choice offered by a provider permission review. */
export const PermissionDenialSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedForOfferedPermissionReviews: "supported_for_offered_permission_reviews",
} as const
export type PermissionDenialSupport =
  (typeof PermissionDenialSupport)[keyof typeof PermissionDenialSupport]
/** Whether suppression of user-configured provider hooks has been verified. */
export const NativeHookSuppressionSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedForUserConfiguredHooks: "supported_for_user_configured_hooks",
} as const
export type NativeHookSuppressionSupport =
  (typeof NativeHookSuppressionSupport)[keyof typeof NativeHookSuppressionSupport]
/** Whether this launch allows the harness's own hooks. Off asks the harness to leave them off. On means hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts. On does not prove a hook ran, and it does not change nativeHookSuppression. */
export const NativeHooksChoice = { Off: "off", On: "on" } as const
export type NativeHooksChoice = (typeof NativeHooksChoice)[keyof typeof NativeHooksChoice]
/** A harness whose own hooks the caller can allow. OpenCode is not in this set and stays suppressed. */
export const NativeHooksAgent = { Claude: "claude", Codex: "codex" } as const
export type NativeHooksAgent = (typeof NativeHooksAgent)[keyof typeof NativeHooksAgent]
/** Whether Nessa reports provider compaction with invocation correlation. */
export const CompactionReportingSupport = {
  UnsupportedNotImplemented: "unsupported_not_implemented",
  SupportedWithInvocationCorrelation: "supported_with_invocation_correlation",
} as const
export type CompactionReportingSupport =
  (typeof CompactionReportingSupport)[keyof typeof CompactionReportingSupport]
/** Whether Nessa reports a provider model switch after validation. */
export const ModelSwitchReportingSupport = {
  UnsupportedNotImplemented: "unsupported_not_implemented",
  SupportedAfterValidatedSwitch: "supported_after_validated_switch",
} as const
export type ModelSwitchReportingSupport =
  (typeof ModelSwitchReportingSupport)[keyof typeof ModelSwitchReportingSupport]
/** Whether a review can be deferred for a later answer. unsupported means this provider cannot defer. unsupported_not_implemented means Nessa has not built deferral for a provider that might. */
export const PermissionDeferralSupport = {
  UnsupportedNotImplemented: "unsupported_not_implemented",
  Unsupported: "unsupported",
  SupportedWithNonterminalOutcome: "supported_with_nonterminal_outcome",
} as const
export type PermissionDeferralSupport =
  (typeof PermissionDeferralSupport)[keyof typeof PermissionDeferralSupport]
/** Whether the provider binding forwards one correlated elicitation round trip. */
export const ElicitationForwardingSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedWithCorrelatedRoundTrip: "supported_with_correlated_round_trip",
} as const
export type ElicitationForwardingSupport =
  (typeof ElicitationForwardingSupport)[keyof typeof ElicitationForwardingSupport]
/** Whether a configured Nessa policy can deny a tool held at a permission gate. */
export const PreToolPolicySupport = {
  Unknown: "unknown",
  UnsupportedNotImplemented: "unsupported_not_implemented",
  SupportedAtPermissionGate: "supported_at_permission_gate",
} as const
export type PreToolPolicySupport =
  (typeof PreToolPolicySupport)[keyof typeof PreToolPolicySupport]
/** Whether a Nessa policy can end its correlated current invocation. */
export const PolicyEndTurnSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedForCurrentInvocation: "supported_for_current_invocation",
} as const
export type PolicyEndTurnSupport =
  (typeof PolicyEndTurnSupport)[keyof typeof PolicyEndTurnSupport]
/** Whether a Nessa policy can close its correlated provider session. */
export const PolicyCloseSessionSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedForSession: "supported_for_session",
} as const
export type PolicyCloseSessionSupport =
  (typeof PolicyCloseSessionSupport)[keyof typeof PolicyCloseSessionSupport]
/** Whether the negotiated agent can ask and receive a correlated answer through Nessa. */
export const IncomingElicitationSupport = {
  Unknown: "unknown",
  Unsupported: "unsupported",
  SupportedWithCorrelatedRoundTrip: "supported_with_correlated_round_trip",
} as const
export type IncomingElicitationSupport =
  (typeof IncomingElicitationSupport)[keyof typeof IncomingElicitationSupport]
/** Scoped provider transport facts and effective Nessa policy integration support. */
export interface ConversationAgentFeatures {
  /** Scoped delivery support for a rejecting option offered by a provider review. */
  permissionDenial: PermissionDenialSupport
  /** Verified suppression state for user-configured provider hooks. */
  nativeHookSuppression: NativeHookSuppressionSupport
  /** The hooks choice this conversation was started with. When on, hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts. For Claude, on also applies the repo's .claude settings, including its allow rules and startup hooks. */
  nativeHooks: NativeHooksChoice
  /** Effective Nessa support for correlated compaction reporting. */
  compactionReporting: CompactionReportingSupport
  /** Effective Nessa support for validated model-switch reporting. */
  modelSwitchReporting: ModelSwitchReportingSupport
  /** Effective Nessa support for an explicit nonterminal permission defer outcome. */
  permissionDeferral: PermissionDeferralSupport
  /** Provider forwarding support for a correlated elicitation round trip. */
  elicitationForwarding: ElicitationForwardingSupport
  /** Effective Nessa support for enforcing configured policy before a held tool runs. */
  preToolPolicy: PreToolPolicySupport
  /** Effective Nessa support for ending a current invocation from policy. */
  policyEndTurn: PolicyEndTurnSupport
  /** Effective Nessa support for closing a provider session from policy. */
  policyCloseSession: PolicyCloseSessionSupport
  /** Effective Nessa support for receiving and resolving incoming elicitation. */
  incomingElicitation: IncomingElicitationSupport
}
/** Operations supported by this configured agent. */
export interface ConversationCapabilities {
  /** Can queue input at invocation boundaries. */
  queue: boolean
  /** Can submit supported steering input. */
  steer: boolean
  /** Can restore the provider context. */
  resume: boolean
  /** Can review provider permission requests. */
  permissions: boolean
  /** The connected agent advertised image input. False until an agent has been opened, and whenever it advertised none. */
  imageInput: boolean
  /** Scoped transport and effective application support for agent features. */
  agentFeatures: ConversationAgentFeatures
}
/** One uploaded image a message refers to. The bytes travel on the upload path, never in a socket message. */
export interface ImageAttachment {
  /** SHA-256 of the image bytes: `sha256:` and 64 lowercase hexadecimal digits. */
  digest: string
  /** Image encoding. */
  mimeType: "image/png" | "image/jpeg" | "image/gif" | "image/webp"
  /** Image length in bytes, at most 5 MiB. */
  size: number
}
/** One original stored file input. Placement is resolved where the agent executes; the original reference is retained for retries and restoration. */
export interface UploadedFileAttachment {
  /** SHA-256 of the bytes to be uploaded; the upload is refused unless the received bytes hash to it. */
  digest: string
  /** Declared lowercase media type without parameters. Storage accepts any; what a message may refer to is narrower. */
  mimeType: string
  /** Exact length in bytes, at most 64 MiB, which admits a camera RAW file; the upload is refused unless it is exactly this long. */
  size: number
  /** Original filename, at most 255 UTF-8 bytes. No separators or control characters; not a destination path. */
  name: string
}
/** One file a message points the agent at. The gateway, the agent and the panel run on one machine, so the path travels and the bytes never do; the agent opens the file itself, with the reader's approval, or does not open it at all. */
export interface LinkedFile {
  /** Absolute path on the machine the gateway runs on, at most 4096 UTF-8 bytes (`x-utf8MaxBytes`; the `maxLength` beside it counts code points and is only a coarse upper bound, since a path within the byte limit always has fewer code points than bytes). Every component below the root is a name: no control character (C0 or C1), none empty, and none `.` or `..`. A component that is empty or a dot does not survive being written as a URI, which would make the link name a different path; a control character makes a path nobody can be shown before they approve the read. Nothing here is a rule about markdown — the gateway hands the path to the agent inside a link and encodes both halves of it down to an allowlist, so a bracket or a backslash in a name is the encoder's business and not the caller's. */
  path: string
}
/** Ask to upload one file into a conversation. Repeating it for bytes the conversation already holds needs no upload. */
export interface AttachmentBeginParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** SHA-256 of the bytes to be uploaded; the upload is refused unless the received bytes hash to it. */
  digest: string
  /** Declared lowercase media type without parameters. Storage accepts any; what a message may refer to is narrower. */
  mimeType: string
  /** Exact length in bytes, at most 64 MiB, which admits a camera RAW file; the upload is refused unless it is exactly this long. */
  size: number
}
/** Read one range of a file a conversation holds: an artifact its agent published, or an image uploaded into it. Name the file exactly as the conversation's view does. A surface reads the whole file range by range and checks its size and digest itself. */
export interface ArtifactReadParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** SHA-256 of the held file, as the view names it. */
  digest: string
  /** Media type the file is held as, as the view names it. The same bytes held as another type are another file. */
  mimeType: string
  /** Exact length of the held file in bytes. */
  size: number
  /** First byte of the range; less than size. */
  offset: number
  /** Most bytes to answer with. The answer carries fewer only at the end of the file. */
  maxBytes: number
}
/** One range of the held file. Nothing in it is verified: assemble the ranges, then check the whole against the file's size and digest before showing or keeping it. */
export interface ArtifactReadResult {
  /** First byte of this range, as asked. */
  offset: number
  /** The range's bytes, standard base64 with padding: at most maxBytes, and fewer only when the file ends first. */
  data: string
}
/** Either the conversation already holds this upload, with the reference a message uses for it, or a single-use ticket to upload it. A successful `PUT /attachments` answers with the same reference shape. */
export interface AttachmentBeginResult {
  /** The action this answers. */
  requestId: string
  /** `stored` needs nothing further; `upload_required` carries a ticket. */
  state: "stored" | "upload_required"
  /** Secret single-use upload ticket, sent as the `x-nessa-upload-ticket` header of one `PUT /attachments`. Null when stored. Do not log it. */
  ticket: string | null
  /** Unix milliseconds after which the ticket is refused. Null when stored. */
  expiresAtMs: number | null
  /** When stored: digest of what the conversation holds, which is what a message refers to. The gateway may have converted or compressed the upload, so this can differ from the uploaded digest. Null when an upload is required. */
  digest: string | null
  /** When stored: media type of what the conversation holds. Null when an upload is required. */
  mimeType: string | null
  /** When stored: length in bytes of what the conversation holds. Null when an upload is required. */
  size: number | null
}
/** Send one piece of an upload over the session itself, for a surface that cannot reach `PUT /attachments`, such as a phone over its native link. Pieces go in order, one at a time, each starting where the last ended: the piece at offset 0 starts the upload, and the one marked `last` ends it and answers with what the conversation now keeps. As with `PUT /attachments`, the ticket is the whole authority and the first piece spends it. An upload whose pieces stop for longer than the upload deadline is refused. A refused piece ends its upload: begin again for a new ticket. */
export interface AttachmentWriteParams {
  /** The secret single-use ticket `attachment.begin` answered with. Do not log it. */
  ticket: string
  /** First byte of this piece: 0 for the first, then the bytes of every piece before it. */
  offset: number
  /** This piece's bytes, standard base64 with padding: at most 32768 bytes. Only a last piece may be empty. */
  data: string
  /** Whether this piece ends the upload. Its answer carries the stored file. */
  last: boolean
}
/** How much of the upload the gateway has taken, and after the last piece the file the conversation now keeps, which is what `conversation.send` refers to. As with `PUT /attachments`, for an image that is the normalized image, so its digest, type and size can differ from what was sent. */
export interface AttachmentWriteResult {
  /** Bytes taken so far: where the next piece starts. */
  received: number
  /** After the last piece: digest of what the conversation holds. Null before it. */
  digest: string | null
  /** After the last piece: media type of what the conversation holds. Null before it. */
  mimeType: string | null
  /** After the last piece: length in bytes of what the conversation holds. Null before it. */
  size: number | null
}
/** A provider usage limit on a failed turn. resetsAtMs is present only for a timed reset whose instant parsed. autoResuming is present only when this gateway has accepted a one-shot continuation for the turn. */
export interface ConversationUsageLimit {
  /** The harness that reported the limit. OpenCode is not a value. */
  provider: "claude" | "codex"
  /** How the limit can end. timed_reset carries resetsAtMs. user_action needs the person and never carries a reset. unknown is a usage limit with no trustworthy reset. */
  recovery: "timed_reset" | "user_action" | "unknown"
  /** Absolute Unix milliseconds when a timed reset ends, kept only inside 2024-01-01 through 2100-01-01 UTC. */
  resetsAtMs?: number
  /** True only when a continuation for this turn is scheduled or being delivered and the gateway preference is on. */
  autoResuming?: boolean
}
/** One bounded conversation turn; omitted older text is indicated by the enclosing truncated flag. */
export interface ConversationMessage {
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** User input text, whole; empty for a message of images alone. Its only bound is the frame's. */
  userText: string
  /** Images the user sent with this turn, in attachment order. */
  attachments: ImageAttachment[]
  /** Original stored file inputs, in order; at most 64 MiB together. Omitted means none. */
  uploads?: UploadedFileAttachment[]
  /** Files the user pointed this turn at, in attachment order. No bytes were ever carried for them. */
  files: LinkedFile[]
  /** The app that wrote this turn on the person's behalf; absent when the person wrote it. */
  app?: ConversationMessageApp
  /** The verified source turn whose agent wrote this input; absent for a person or app. */
  agent?: ConversationAgentSource
  /** Current invocation state. */
  status: ConversationMessageStatus
  /** Bounded diagnostic for this invocation. */
  error?: string
  /** The provider adapter explicitly requires authentication for this failed turn. Generic provider codes and diagnostic text do not establish this fact. */
  authenticationRequired?: boolean
  /** This failed turn also failed independent required work, beside an authentication refusal or a usage limit. The person-facing diagnostic stays in error. Absent for a pure refusal or a pure usage limit. */
  requiredWorkFailed?: boolean
  /** A provider usage limit on this failed turn. Absent when the turn did not end in one. autoResuming is present only after a continuation has been scheduled. */
  usageLimit?: ConversationUsageLimit
  /** Execution that consumed this injected steering input; its shared reply answers this input. */
  steeringTarget?: string
  /** Provider observations in execution order; offsets address the retained SDK event sequence. */
  parts: ConversationPart[]
  /** Target event count when steering was admitted locally; does not imply provider consumption timing. */
  steeringOffset?: number
  /** Where the first listed part starts in the whole turn, when the view begins partway through it. The person's message and the parts before it are read with conversation.readEarlier. Absent when the turn is listed from its start. */
  startsAt?: ConversationPartPosition
}
/** A waiting input that may be removed before dispatch. */
export interface ConversationPending {
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Waiting user input; empty for a message of images alone. */
  text: string
  /** Images waiting with this input, in attachment order. */
  attachments: ImageAttachment[]
  /** Original stored file inputs, in order; at most 64 MiB together. Omitted means none. */
  uploads?: UploadedFileAttachment[]
  /** Files the waiting input points at, in attachment order. */
  files: LinkedFile[]
  /** The app that wrote this waiting input on the person's behalf; absent when the person wrote it. */
  app?: ConversationMessageApp
  /** The verified source turn whose agent wrote this waiting input. */
  agent?: ConversationAgentSource
  /** Queue or steering admission. */
  mode: ConversationPendingMode
}
/** The source conversation and turn whose agent wrote a message or note. Origin does not grant owner authority. */
export interface ConversationAgentSource {
  /** Canonical ID of the conversation in which the sending agent ran. */
  conversationId: string
  /** ID of the source turn or the child turn a report concerns. */
  executionId: string
}
/** The MCP App that wrote a turn on the person's behalf (mcp.sendMessage): the tool call whose UI it is, and the MCP server and tool that call was to. */
export interface ConversationMessageApp {
  /** The execution the app's tool call belongs to. */
  executionId: string
  /** The app's tool call. */
  toolId: string
  /** The app's MCP server. */
  server: string
  /** The tool whose UI the app is. */
  tool: string
}
/** An exact option offered by the provider. */
export interface ConversationPermissionOption {
  /** Opaque offered option identifier. */
  id: string
  /** Provider label for this option. */
  label: string
  /** What choosing this option decides for the reviewed request, as the gateway's domain classified the provider's offer: allow it, or deny it. A surface picks an option by this, never by its label or identifier. */
  effect: ConversationPermissionOptionEffect
}
/** Whether a permission option allows or denies the reviewed request. Only options that decide that one request are offered: the gateway offers no review with a choice that reaches further. */
export const ConversationPermissionOptionEffect = {
  Allow: "allow",
  Deny: "deny",
} as const
export type ConversationPermissionOptionEffect =
  (typeof ConversationPermissionOptionEffect)[keyof typeof ConversationPermissionOptionEffect]
/** What happens if nobody answers this review, and when. Absent on a review that waits for a person. */
export interface ConversationPermissionDeadline {
  /** How many milliseconds the review waits before this outcome is taken, measured when the review was armed. */
  waitMs: number
  /** Unix milliseconds on the gateway's clock when the deadline decides. A reloaded client counts down from this rather than starting waitMs again, measured from the view's serverNowMs rather than its own clock. */
  dueAtMs: number
  /** The outcome taken when the wait ends with no answer. */
  effect: ConversationPermissionOptionEffect
  /** Past-tense reason recorded if the deadline wins: not blank, and free of control characters. A pending review does not show this sentence; it builds a line from the outcome and the wait. */
  reason: string
  /** True when the only refusal this agent offered stops the turn; never true for an allow. The pending line and the notice say that, rather than that the turn continues without the tool. */
  stopsTurn: boolean
}
/** A pending review with all it shows, as conversation.permissionReview reads it for one entry of a view. */
export interface ConversationPermission {
  /** The key the view lists it under. An answer names it, so a review whose identities are reused is never answered by mistake. */
  review: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Permission identity within the execution. */
  permissionId: string
  /** Tool being reviewed. */
  toolId: string
  /** Provider tool title. */
  title: string
  /** Complete offered choices; never silently shortened. */
  options: ConversationPermissionOption[]
  /** Exact reviewed tool name. */
  toolName: string
  /** Who asked for this review: the agent or an MCP App. */
  origin: ConversationPermissionOrigin
  /** What the review asks the person to allow. */
  ask: ConversationPermissionAsk
  /** The reviewed input, exactly as the agent or the app gave it. */
  argumentsJson: string
  /** Present when the review answers itself if nobody does. Absent for a review that waits for a person, including an app review. */
  deadline?: ConversationPermissionDeadline
}
/** One review waiting on a person, as a view lists it: its key, who asked, and the start of the tool's name. A view never leaves one out for its size. Read what it shows with conversation.permissionReview, and offer no answer until that has loaded. */
export interface ConversationReviewEntry {
  /** The review's own key: new for every review opened, so a review whose identities are reused is a new entry. */
  review: string
  /** Who asked: the agent, or an MCP App. */
  origin: "harness" | "app"
  /** The start of the reviewed tool's name, to say what the review is while the rest is read. */
  toolName: string
}
/** One question waiting on a person, as a view lists it: its key and the start of the agent's message. A view never leaves one out for its size. Read all it asks with conversation.questionReview, and offer no answer until that has loaded. */
export interface ConversationQuestionEntry {
  /** The question's own key: new for every question asked, so a question whose identities are reused is a new entry. */
  question: string
  /** The start of the agent's message, to say what the question is while the rest is read. */
  message: string
}
/** Bounded presentation of an observed tool call. */
export interface ConversationTool {
  /** Gateway-owned background task bound to this original native call. Its running status remains authoritative after the provider turn ends. */
  backgroundTaskId?: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Tool call identity within the execution. */
  toolId: string
  /** Provider tool title. */
  title: string
  /** What the call does, as the provider categorised it. Empty until the provider has said, which is why a panel counts kinds it knows rather than assuming the rest are reads. */
  kind:
    | ""
    | "read"
    | "edit"
    | "search"
    | "fetch"
    | "execute"
    | "think"
    | "delete"
    | "move"
    | "switch_mode"
    | "other"
  /** Current provider tool status. */
  status: string
  /** Bounded provider-observed tool output and file changes; omitted content is marked. */
  details: string
  /** The call's input, one JSON object encoded: the arguments the agent reported with the call, or those its permission request asked with, whichever came last and fit. Empty when none was reported or none fit — never cut. */
  input: string
  /** The MCP server and tool the call went to, as the agent's harness named them (Claude's harness replaces characters outside [A-Za-z0-9_-] in a tool name with _); absent for a harness's own tools and where the harness does not say which server. */
  mcp?: ConversationMcpTool
  /** The call's structured result (MCP structuredContent) as JSON text, when the harness passed it on and it fits; absent otherwise. The result's text stays in details either way. */
  structuredContent?: string
  /** The hooks choice of the process start this tool call ran under, saved with the call so it survives a gateway restart. Absent when that start recorded none: a harness without the choice, or a call saved before it was recorded. Recorded input and output are what the model proposed or the tool returned, and a hook may have changed or approved them when this is on. */
  nativeHooks?: NativeHooksChoice
  /** When the gateway first heard of the call, in Unix milliseconds, saved with the call so it survives a restart. Absent for a call saved before the time was recorded. */
  startedAtMs?: number
  /** When the gateway heard the call completed or failed, in Unix milliseconds. Absent while the call runs, for a call that never settled, and for a call saved before the time was recorded. */
  endedAtMs?: number
  /** The files the call changed, as the agent reported them and in its order, at most 16. The same changes are written into details as text; these are for a reader that draws them, and the first a view gives up for room, before any turn. Absent when the call reported none. */
  changes?: ConversationToolChange[]
  /** The call changed files changes does not list: more than 16, one whose path is past its bound, or the list was given up for room. A reader totalling changes knows the total is not the call's. Absent when changes lists every file the call reported. */
  changesCut?: boolean
}
/** One file a tool call changed: its path, and its text before and after. */
export interface ConversationToolChange {
  /** The file's path as the agent named it, unresolved. A change to a file whose path is longer than 4096 bytes is not listed, and changesCut says so. */
  path: string
  /** The text before the change, cut past 7000 bytes. Absent when the agent did not say — a file it created reads that way — which differs from an empty file. */
  before?: string
  /** The text after the change, which may be empty, cut past 7000 bytes. */
  after: string
  /** A side was longer than 7000 bytes and is cut, so the two sides do not say the whole change. */
  cut: boolean
}
/** An MCP tool's identity: the server by the name it was configured under, and the tool on it; the UI the tool declared, when known; and the arguments the gateway's connection saw for the call, when it could match them. */
export interface ConversationMcpTool {
  /** The MCP server's configured name. Which names are valid is the SDK domain's rule (McpTool); only its byte bound is repeated here, generated for both sides. */
  server: string
  /** The tool's name on that server, under the same rule. */
  tool: string
  /** The ui:// resource of the tool's MCP App, as the gateway's own connection to the server last listed the tool (no harness passes it through ACP); absent when the tool declared none or it is not known. Which URIs are valid is the SDK domain's rule (UiResourceUri); only its byte bound is repeated here. */
  resourceUri?: string
  /** The call's arguments, one JSON object encoded, as the gateway's MCP connection saw them. The same bound as a review and an app's own call (maxMcpArgumentsBytes). Absent when the call did not go through that connection, the harness named no call id it could match, or the arguments did not fit — never cut. */
  argumentsJson?: string
}
/** Bounded full replacement of the current live conversation view. Polling never implies cancellation or durable streaming storage. */
export interface ConversationView {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Opaque revision; compare for equality, never numeric ordering. */
  revision: string
  /** The gateway's Unix milliseconds when it made this view. A client measures each deadline's dueAtMs against this, not against its own clock, so a clock on another machine that is ahead or behind does not move the countdown. */
  serverNowMs: number
  /** Recent turns with current streamed output. */
  messages: ConversationMessage[]
  /** Current removable queue entries. */
  pending: ConversationPending[]
  /** Every review waiting on a person now, oldest first: the agent's (the SDK holds at most MAX_OPEN_REVIEWS open) and then the MCP Apps'. Each entry is small and of fixed size, and a bounded view gives up everything else before any of them, so none is ever left out. */
  permissions: ConversationReviewEntry[]
  /** Recent tool states. */
  tools: ConversationTool[]
  /** Agent operation support. */
  capabilities: ConversationCapabilities
  /** Current provider attachment lifecycle. */
  lifecycle: ConversationLifecycle
  /** Something no read gives back was omitted to bound this response: a gap between the turns listed, notes, artifacts, or whole interactions; interactionViewError explains omitted actionable units. Older turns that only come before the first one listed are earlier instead. */
  truncated: boolean
  /** Turns, or the start of the first turn listed, came before what this view lists. They are read with conversation.readEarlier, before the first message's start. */
  earlier: boolean
  /** Why complete pending reviews or questions cannot safely be shown; do not infer omitted choices or question fields. */
  interactionViewError?: string
  /** All currently pending execution IDs are represented, so an exact reorder may be attempted. */
  queueComplete: boolean
  /** Committed physical transcript completeness and freshness, independent of display truncation. */
  transcriptState:
    "not_loaded" | "partial" | "complete_empty" | "complete" | "stale" | "unknown"
  /** Configured provider, model and working directory for this conversation. */
  runtime?: ConversationRuntime
  /** The conversation's latest lease: where its agent runs and under what limits, or why it could not. Absent before any lease was recorded. */
  lease?: ConversationLease
  /** Files its agent published under a lease that the conversation held when it recorded them, the newest 64 in the order they were recorded; empty before any was. A record is history and stays when the hold is let go, as closing the conversation does; whether the bytes are still held is asked when they are fetched. An older one is left out of the list, not out of the conversation, and the list may give up its oldest entries to bound the response. */
  artifacts: ConversationArtifact[]
  /** Notes left in the conversation (conversation.message), the newest 64 in the order they were left; empty before any was. An older one is left out of the list, not out of the conversation, and the list may give up its oldest entries to bound the response. */
  notes: ConversationNote[]
  /** Switches to another agent or model, and the fork the conversation began as, in the order they were recorded; empty before any. */
  handoffs: ConversationHandoff[]
  /** Records this read could not apply, oldest first, at most 32. Empty when every record could be read. A chat with no turns may still list these. Each is one line in the transcript. The line is where a future report action attaches; this view carries no report control. */
  unreadable: ConversationUnreadable[]
  /** True when more records could not be read than the 32 lines in unreadable. Those records were still skipped. The transcript ends with one line: More parts of this conversation couldn't be read. */
  unreadableTruncated: boolean
  /** Name the gateway derived from the conversation's first message, or the one a person renamed it to, the same one conversation.list shows; null before anything was said. */
  title: string | null
  /** Every question the agent is waiting on now, oldest first (the SDK holds at most MAX_OPEN_QUESTIONS open). Each entry is small and of fixed size, and a bounded view gives up everything else before any of them, so none is ever left out. */
  questions: ConversationQuestionEntry[]
  /** Last durably committed preset. */
  approvalMode: ApprovalMode
  /** Presets supported by this model. */
  approvalModes: ApprovalModeChoice[]
  /** Present while a change is in progress or recovery is required. */
  approvalModeChange?: ApprovalModeChange
  /** Pending correlated model change; the confirmed selection remains authoritative. */
  modelChange?: ModelChange
  /** Whether this gateway schedules one continuation when a usage limit resets. The preference is the gateway's, shared by every conversation, and defaults to false. The card copies Auto-resuming only when a schedule for that turn has been accepted. */
  usageLimitAutoContinue?: boolean
  /** Native effect boundary of the last confirmed model change. */
  modelChangeEffect?: ModelChangeEffect
  /** The conversation's selected catalog model, present whether or not an agent is open. */
  model: string
}
/** One file its agent published under a lease: read from the environment, verified by its digest, held, and then recorded with the lease as its cause. The record outlives the hold. The bytes are never in the view. */
export interface ConversationArtifact {
  /** The turn that was running when it was recorded; absent when none was. */
  executionId?: string
  /** The lease it was published under. Which identities are valid is the SDK domain's rule (LeaseId); only its byte bound is repeated here. */
  lease: string
  /** The file name it was published under, for people only: it names no place on any machine. Which names are valid is the SDK's rule (ArtifactName: no `/` and no control character); only its byte bound is repeated here. */
  name: string
  /** SHA-256 of its bytes: `sha256:` and 64 lowercase hexadecimal digits. */
  digest: string
  /** The media type it was declared as, lowercase without parameters. Which types are valid is the SDK domain's rule (MediaType); only its byte bound is repeated here. */
  mimeType: string
  /** Its length in bytes, at most 64 MiB. */
  size: number
}
/** Idempotently create or reopen one named conversation. */
export interface ConversationCreateParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Coding agent this conversation runs on for its whole life. Omitted takes the gateway's configured default. Ignored when the conversation already exists, which is reopened on the agent it was created with. */
  agent?: string
  /** Initial catalog model identifier; ignored on reopen. Use conversation.setModel to change an existing chat. */
  model?: string
  /** Initial preset, default ask; ignored on reopen. */
  approvalMode?: ApprovalMode
  /** Where this conversation's agent runs for its whole life: one of the environments agents.list returns, an SSH destination, a paired environment's peer: name, or peer:any, any worker of the gateway's pool, which the gateway chooses each time the agent is opened. Omitted runs it on the gateway's own machine. Ignored when the conversation already exists, which keeps the environment it was created with. */
  environment?: string
  /** The channel the conversation is in for its whole life, one channel.list returns: its agent works in that channel's folder and is told the channel's notes and resources. Omitted, or default, is the default channel, whose folder is the gateway's configured workspace. A channel other than default runs on the gateway's own machine, so it is refused invalid_request beside an environment. channel_not_found when the caller has no such channel. Ignored when the conversation already exists, which stays in the channel it was created in. */
  channelId?: string
}
/** Conversation ready for read and admission. */
export interface ConversationCreateResult {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
}
/** Capture the locally active execution without opening a provider. Null means no active execution; bounded transcript views are not used to select it. */
export interface ConversationPeekResult {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  executionId: string | null
}
/** A place in a turn: the part, counted from the turn's first, and the byte within that part's text. */
export interface ConversationPartPosition {
  /** The part, counted from the turn's first part. */
  part: number
  /** The UTF-8 byte within that part's text where the place falls; 0 at the part's start. */
  byte: number
}
/** A place in a conversation's history: the start of a turn, before the person's message, when at is absent; otherwise a place among the turn's parts, after the person's message. */
export interface ConversationPoint {
  /** The turn the point is in. */
  executionId: string
  /** A place among the turn's parts, after the person's message; absent for the turn's start, before it. */
  at?: ConversationPartPosition
}
/** Read what came before a point in the conversation's history: the turns nearest it that fit one frame. With after, only what comes after that point, to fill a gap between two points. */
export interface ConversationReadEarlierParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Where what is read ends: the first point the reader already holds. */
  before: ConversationPoint
  /** Where what is read starts, when the reader holds what comes before it; absent to read back toward the first turn. */
  after?: ConversationPoint
}
/** What came before a point, oldest first, ending where the point is. The first message starts partway through its turn when it carries startsAt. */
export interface ConversationEarlier {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The turns before the point, oldest first; the first may start partway (startsAt) and the last may end partway, where the point falls. */
  messages: ConversationMessage[]
  /** The calls the messages made. */
  tools: ConversationTool[]
  /** The fork the conversation began as, with its copied turns, on the page that reaches the start; empty otherwise. */
  handoffs: ConversationHandoff[]
  /** Something comes before the first message's start (after the after point, when one was given). */
  more: boolean
  /** The page left out something no read gives back, as a view's truncated says. */
  truncated: boolean
}
/** Read the current bounded projection. */
export interface ConversationReadParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
}
/** Which of the caller's conversations to list: those not archived, unless archived ones are asked for. */
export interface ConversationListParams {
  /** List only archived conversations when true; only unarchived ones when false or absent. */
  archived?: boolean
}
/** One conversation as a list row: what it is called, the last thing said in it, and when. Read from stored summaries and live state; listing never opens or resumes a provider. */
export interface ConversationSummary {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Name the gateway derived from the conversation's first message, or the one a person renamed it to, the same one its view carries; null when none is on record. */
  title: string | null
  /** The last thing said in the conversation, as one line of plain text, or null when there is none on record. */
  preview: string | null
  /** When the conversation was created, in Unix milliseconds, as its creator requested it. */
  createdAtMs: number
  /** When something was last said in the conversation, in Unix milliseconds; the creation time until then. */
  updatedAtMs: number
  /** The conversation is open on this gateway and an invocation is in progress. */
  running: boolean
  /** Somebody archived the conversation and nothing has been said in it since. */
  archived: boolean
  /** The channel the conversation was created in, one channel.list returns. Absent for a conversation in the default channel. */
  channelId?: string
}
/** Conversations the authenticated caller owns, newest first, at most 500, and whether that is all of them. */
export interface ConversationListResult {
  /** The caller's conversations, most recently updated first. Ownership is applied before the bound, so another principal's conversations never take a place in it. */
  conversations: ConversationSummary[]
  /** True when `conversations` names every conversation the caller has under this list's filter. False when the 500 bound left some out, or when a stored record or summary of the caller's own cannot be read back, which lasts until an operator repairs it. Nobody else's conversations are read to answer a list, so nobody else's damage makes it false. A conversation missing from a complete list is not there under that filter: deleted, listed under the other filter, or one the gateway has no summary for (nothing was said in it, or its summary was never written). */
  complete: boolean
}
/** Where an observation pass resumes: the same incarnation and boundary, then the next descriptor after this creation revision and identity. */
export interface ConversationObserveCursor {
  /** Catalogue database incarnation captured with this pass. A later pass starts again when it no longer matches. */
  incarnation: string
  /** Fixed owner head captured when this pass began. A conversation created after it waits for the next pass. */
  boundary: string
  /** Creation revision of the last descriptor this pass has already returned. */
  creation: string
  /** Conversation identity of that last descriptor. Compared only when the creation revision is the same. */
  id: string
}
/** One page of the caller's conversation catalogue. Opens no provider. Does not raise the 500-row list. */
export interface ConversationObserveParams {
  /** Observe only archived conversations when true; only unarchived ones when false or absent. The same filter as conversation.list. */
  archived?: boolean
  /** Absent on the first page. Later pages echo the cursor the previous page returned. */
  cursor?: ConversationObserveCursor
}
/** One catalogue page of the caller's summaries, and whether the pass is finished. The page size is the catalogue bound, not a larger conversation.list. */
export interface ConversationObserveResult {
  /** Summaries from this catalogue page whose archived flag is the one asked for, creation order. At most one catalogue page. Deleted rows, rows with no summary, and the other archived flag are left out of this array and still advance the cursor. Ownership is applied before the page is returned. */
  conversations: ConversationSummary[]
  /** True only when this page finishes the pass: every stored summary under the filter, up to the captured head, was returned across the pages. False when further descriptors remain, or when this page cannot say where to resume. A false page is not every stored summary, including when it has no cursor. */
  complete: boolean
  /** Present when complete is false and another page can be asked. Absent when the pass is finished, or when this page cannot resume. */
  cursor?: ConversationObserveCursor
}
/** Submit one input; execution and request IDs stay fixed across retries. */
export interface ConversationSendParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** User message, at most 8 KiB UTF-8; gateway enforces the byte bound. May be blank only when attachments are present. */
  text: string
  /** Images already uploaded into this conversation, in attachment order; at most 10 MiB in total. Empty for a message of text alone. */
  attachments: ImageAttachment[]
  /** Original stored file inputs, in order; at most 64 MiB together. Omitted means none. */
  uploads?: UploadedFileAttachment[]
  /** Files on this machine the message points the agent at, in attachment order. Nothing is uploaded for them and nothing is read here. Empty for a message that points at none. */
  files: LinkedFile[]
}
/** Remove an input that has not dispatched. */
export interface ConversationRemoveParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
}
/** Stop one captured turn. A queued turn is withdrawn. The active turn is cancelled without closing the attachment. A finished turn records that nothing was sent. */
export interface ConversationStopParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The captured turn. Stop never substitutes a newer one. */
  executionId: string
}
/** Which command family a read-only receipt lookup names. The lookup does not admit that command. */
export const ConversationCommandOperation = {
  Create: "create",
  Submit: "submit",
  Steer: "steer",
  Stop: "stop",
} as const
export type ConversationCommandOperation =
  (typeof ConversationCommandOperation)[keyof typeof ConversationCommandOperation]
/** Durable progress. Attempted means the original effect was authorized and its success was not saved. Ready is creation's terminal. Settled is a submit or stop terminal. */
export const ConversationCommandStage = {
  Accepted: "accepted",
  Attempted: "attempted",
  Ready: "ready",
  Settled: "settled",
} as const
export type ConversationCommandStage =
  (typeof ConversationCommandStage)[keyof typeof ConversationCommandStage]
/** What a settled submit or stop did. Absent until the stage is settled. Already final means nothing was sent. No effect means the submit was refused before anything was enqueued or injected; retrying that request answers the same refusal and does not dispatch. Refused after save means the submit saved the turn and then refused before injection; retrying that request answers revoked_after_save and does not inject. Unavailable after save means the submit saved the turn and then could not decide before injection; retrying that request answers unavailable_after_save and does not inject. */
export const ConversationCommandOutcome = {
  Dispatched: "dispatched",
  Withdrawn: "withdrawn",
  Cancelled: "cancelled",
  AlreadyFinal: "already_final",
  NoEffect: "no_effect",
  RefusedAfterSave: "refused_after_save",
  UnavailableAfterSave: "unavailable_after_save",
} as const
export type ConversationCommandOutcome =
  (typeof ConversationCommandOutcome)[keyof typeof ConversationCommandOutcome]
/** Non-content progress of one creation, submit, or stop. The request identity, not this object, is the idempotency key. */
export interface ConversationCommandReceipt {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Acknowledged progress of that command. */
  stage: ConversationCommandStage
  /** Present only when stage is settled. */
  outcome?: ConversationCommandOutcome
}
/** Read one command receipt. This writes nothing and does not enqueue, open, withdraw, or cancel. Submit and steer must repeat the original bytes so the fingerprint matches. Create repeats the original selection hints. */
export interface ConversationReceiptParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Command family to read. A different family under this request is a conflict. */
  operation: ConversationCommandOperation
  /** Required for submit, steer, and stop. The captured turn. */
  executionId?: string
  /** Original submit or steer text. Required for those operations, omitted for create and stop. */
  text?: string
  /** Original submit or steer images, in attachment order. Omitted means an empty list, the same fingerprint as a send that carried none. */
  attachments?: ImageAttachment[]
  /** Original stored file inputs, in order; at most 64 MiB together. Omitted means none. */
  uploads?: UploadedFileAttachment[]
  /** Original submit or steer file paths, in attachment order. Omitted means an empty list, the same fingerprint as a send that carried none. */
  files?: LinkedFile[]
  /** Creation hint. Omitted when the original creation named no agent. */
  agent?: string
  /** Creation hint. Omitted when the original creation named no model. */
  model?: string
  /** Creation hint. Omitted when the original creation named no preset. */
  approvalMode?: ApprovalMode
  /** Creation hint. Omitted when the original creation named no environment. */
  environment?: string
  /** Creation hint. Omitted when the original creation named no channel. */
  channelId?: string
}
/** Whether a receipt for that request is saved. Found is false when the principal has no such request. A deleted conversation is refused instead of answering found. */
export interface ConversationReceiptResult {
  /** True when this request has saved progress. */
  found: boolean
  /** The request that was looked up. */
  requestId: string
  /** Present when found is true. */
  stage?: ConversationCommandStage
  /** Present when the saved stage is settled. */
  outcome?: ConversationCommandOutcome
}
/** Whether a failed permission answer left the domain review pending, consumed it, or could not prove either state. */
export const ConversationPermissionSelectionState = {
  Pending: "pending",
  Consumed: "consumed",
  Unknown: "unknown",
} as const
export type ConversationPermissionSelectionState =
  (typeof ConversationPermissionSelectionState)[keyof typeof ConversationPermissionSelectionState]
/** Typed review state attached to a failed conversation.answer response. This state is independent of the diagnostic error code. */
export interface ConversationPermissionAnswerErrorDetails {
  /** Authoritative knowledge of whether the reviewed option was selected. */
  selectionState: ConversationPermissionSelectionState
}
/** Choose one option from the exact pending permission. */
export interface ConversationAnswerParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** The review's key, as the view lists it. Refused with stale_permission when the review waiting under these identities has another. */
  review: string
  /** Pending permission identity. */
  permissionId: string
  /** Exact provider-offered option identity. */
  optionId: string
}
/** Read one range of what a waiting review shows: the UTF-8 JSON of its ConversationPermission, from the live owner of the review. A surface reads from 0 until totalBytes, checks the whole is that long, and offers no answer until it has. Refused with stale_permission once the view no longer lists the review. */
export interface ConversationPermissionReviewParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The review's key, as the view lists it. */
  review: string
  /** First byte of the range: 0, or less than the review's length. */
  offset: number
  /** Most bytes to answer with. The answer carries fewer only at the end of the review. */
  maxBytes: number
}
/** Read one range of all a waiting question asks: the UTF-8 JSON of its ConversationQuestion, from the agent's live invocation. A surface reads from 0 until totalBytes, checks the whole is that long, and offers no answer until it has. Refused with stale_permission once the view no longer lists the question. */
export interface ConversationQuestionReviewParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The question's key, as the view lists it. */
  question: string
  /** First byte of the range: 0, or less than the question's length. */
  offset: number
  /** Most bytes to answer with. The answer carries fewer only at the end of the question. */
  maxBytes: number
}
/** One range of what a waiting review shows. Assemble the ranges, check the whole is totalBytes long, is UTF-8 and is the ConversationPermission of the review asked for, then show it. */
export interface ConversationPermissionReviewResult {
  /** First byte of this range, as asked. */
  offset: number
  /** Length of the whole review, in UTF-8 bytes. */
  totalBytes: number
  /** The range's bytes, standard base64 with padding: at most maxBytes, and fewer only when the review ends first. */
  data: string
}
/** One range of all a waiting question asks. Assemble the ranges, check the whole is totalBytes long, is UTF-8 and is the ConversationQuestion of the question asked for, then show it. */
export interface ConversationQuestionReviewResult {
  /** First byte of this range, as asked. */
  offset: number
  /** Length of the whole question, in UTF-8 bytes. */
  totalBytes: number
  /** The range's bytes, standard base64 with padding: at most maxBytes, and fewer only when the question ends first. */
  data: string
}
/** Cancel one permission review with a caller reason. */
export interface ConversationCancelParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** The review's key, as the view lists it. Refused with stale_permission when the review waiting under these identities has another. */
  review: string
  /** Pending permission identity. */
  permissionId: string
  /** Reason recorded with authenticated caller attribution. */
  reason: string
}
/** Close the live provider context while retaining conversation history. */
export interface ConversationCloseParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
}
/** Archive or unarchive a conversation: whether conversation.list shows it by default. Nothing is stopped or removed, and a new message unarchives it. */
export interface ConversationArchiveParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
}
/** Rename a conversation: the title conversation.list and a read show from now on. A later message does not rename it back. applied is false when it already has that title, and for a conversation nothing was said in, which has no title to change. */
export interface ConversationRenameParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The new title: one line, with no whitespace at either end and no run of more than one whitespace character. Anything else is refused invalid_request; normalize before sending. */
  title: string
}
/** Delete a conversation permanently: stop it, ask its agent to delete the agent's own session, record who deleted it, and erase its history, uploads and summary. Audit evidence is retained. The identity is never reused: its owner's later commands on it answer conversation_deleted, except deleting it again, and anyone else is told conversation_not_found. A repeat of the deciding request — the same principal, surface and requestId — is answered applied true, any other delete by its owner applied false; either repeat first carries on an erasure that has not finished. conversation_erasure_incomplete and audit_unavailable mean the conversation is deleted and its erasure did not finish; deleting again, and each gateway start, tries again. Any other error from a delete means only that it is not known whether the conversation was deleted: list it, or delete again. */
export interface ConversationDeleteParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
}
/** One capability a conversation grant may include. read: list, read and follow the conversation's records. comment: leave notes with conversation.message and ask where its own note stands with conversation.messageStatus. drive: start, steer and stop turns, answer or cancel their permission requests, answer their questions and look up those commands' receipts (conversation.send, steer, stop, answer, answerQuestion, cancel, receipt), with text only and under the grant's constraints. The only sets are read; read and comment; and read, comment and drive. Comment and drive are refused for a gateway principal. No capability deletes, archives, exports, re-shares or changes the model: those stay the owner's. */
export const ConversationCapability = {
  Read: "read",
  Comment: "comment",
  Drive: "drive",
} as const
export type ConversationCapability =
  (typeof ConversationCapability)[keyof typeof ConversationCapability]
/** A named tool policy. read_only_tools: only tools that read. no_shell: every tool except shell commands. ask_everything: every tool, each one asked for. all: every tool, asked as the conversation's approval mode asks. */
export const ConversationToolPreset = {
  ReadOnlyTools: "read_only_tools",
  NoShell: "no_shell",
  AskEverything: "ask_everything",
  All: "all",
} as const
export type ConversationToolPreset =
  (typeof ConversationToolPreset)[keyof typeof ConversationToolPreset]
/** The tool policy a drive grant's turns run under: either preset alone, or allow and deny together. Allow and deny each hold at most 16 patterns, and at least one of the two holds one (both empty is refused); each pattern is 1 to 48 UTF-8 bytes with no surrounding whitespace and no control character; anything else is refused invalid_request. A pattern's * matches any run of characters and every other character is itself; a pattern matches a tool's name as the agent or the gateway names it (Read, read, run), and a deny wins over an allow. Every tool call a drive turn's agent asks permission for, and the gateway's own run, artifacts_publish and artifacts_fetch, is checked against the policy at the revision the turn was admitted under; a refused call is answered as refused and the turn goes on. An allowed run goes only to a host commandHosts names and the gateway grants commands on, and runs only a program the gateway allows. One thing a policy does not limit: what the agent does in the ask approval mode without asking (Claude reads files; Codex runs commands in its read-only, offline sandbox), so a deny pattern naming a read tool does not stop those reads. */
export interface ConversationToolPolicy {
  /** A named policy. Present alone, without allow or deny. */
  preset?: ConversationToolPreset
  /** Tools the holder's turns may call. Present with deny, without preset. */
  allow?: string[]
  /** Tools the holder's turns may never call. Present with allow, without preset. */
  deny?: string[]
  /** The hosts the holder's turns may run commands on with run, by the name environments_list gives them: an SSH destination or a paired environment's peer: name, never here, each named once and at most 253 bytes, the longest destination config.json accepts. Beside either preset or allow and deny, and not counted against the patterns: a policy may name 16 hosts however full its allow and deny lists are. A host is used only where the gateway also grants commands on it. Absent or empty, the holder's turns run commands nowhere: a drive grant never inherits the owner's hosts. A host that is no such name, one named twice, one longer than 253 bytes, or more than 16 is refused invalid_request. Listed only when it names a host. */
  commandHosts?: string[]
}
/** Grant (conversation.share) one paired device or peer gateway a capability set on one of the caller's conversations, or change the capabilities and constraints it already holds there. Nothing is granted by default or by list: a paired party sees only the conversations granted to it, one grant per conversation, and an ungranted one is not listed, not readable and cannot be followed. The only sets are read; read and comment; and read, comment and drive. drive needs a policy and the others take none, and either mistake is refused invalid_request. Comment and drive are written only when Cedar permits grant.comment and grant.drive for the target principal. A denial writes nothing and is refused capabilities_refused; details.policies are the Cedar policy ids, empty when no permit matched. The share does not decide by the principal's kind. A grant is refused share_target_not_paired unless the credential is an active paired device or peer gateway of the conversation's owner that holds conversation.read (an environment's credential never does), and conversation_deleted on a deleted conversation. The result names the grant's policyRevision, the grant epoch, once the share is made; applied is false when nothing changed: granting the capabilities and constraints already held, whose epoch is kept. With expectedPolicyRevision, a grant that is no longer at that epoch (changed or revoked since) is refused share_grant_changed, and nothing is changed. A conversation holds at most 64 grants, and at most 8 drive grants; one more is refused invalid_request. A retried requestId answers what it answered the first time, its policyRevision included, before expectedPolicyRevision is compared, so a share retried after a later unshare grants nothing; the same requestId naming another device, another capability set or policy, or the other change is refused invalid_request. A retried share is checked like a new one first, so once the conversation is deleted or the device unpaired it is refused conversation_deleted or share_target_not_paired and changes nothing. */
export interface ConversationShareParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The paired device's credential, as credential.list names it. */
  credentialId: string
  /** The capabilities to grant, in any order. read; or read and comment; or read, comment and drive. */
  capabilities: ConversationCapability[]
  /** The constraints: the tool policy. Present exactly when capabilities includes drive. */
  policy?: ConversationToolPolicy
  /** The policyRevision of the grant this change was made from, as conversation.shares or an earlier share's result named it. When given, the change is made only while the device's grant is still at that revision, decided in the same transaction that writes it; otherwise it is refused share_grant_changed and nothing is changed. 0 names no grant held (revisions start at 1): a first grant built on a list without one is refused once another lands. Absent, the change is made whatever the device holds. */
  expectedPolicyRevision?: number
}
/** Revoke (conversation.unshare) one paired party's grant on one of the caller's conversations, whatever its capabilities. A revoke needs only ownership, so it works on a deleted conversation and for a party since unpaired. Revoking ends the party's next read; a read already admitted may finish. applied is false when no grant was held. Either way the party holds no grant afterwards, so the result names no revision: the revoked grant's row is gone, and a later grant takes an epoch of its own. With expectedPolicyRevision, a grant that is no longer at that epoch (changed or revoked since) is refused share_grant_changed, and nothing is changed. A retried requestId answers what it answered the first time; the same requestId naming another device or the other change is refused invalid_request. */
export interface ConversationUnshareParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The paired device's credential, as credential.list names it. */
  credentialId: string
  /** The policyRevision of the grant this change was made from, as conversation.shares or an earlier share's result named it. When given, the change is made only while the device's grant is still at that revision, decided in the same transaction that writes it; otherwise it is refused share_grant_changed and nothing is changed. 0 names no grant held (revisions start at 1): a first grant built on a list without one is refused once another lands. Absent, the change is made whatever the device holds. */
  expectedPolicyRevision?: number
}
/** Where a conversation.shares page resumes: the last grant the previous page returned. The next page is the grants after it in oldest-first order, grantedAtMs then credentialId. */
export interface ConversationSharesCursor {
  /** When that grant was made, Unix milliseconds, as the previous page listed it. */
  grantedAtMs: number
  /** The paired device's credential, as the previous page listed it. */
  credentialId: string
}
/** One page of the grants on one of the caller's conversations, oldest first. The first page names no cursor. A later page echoes the cursor the previous page returned, and is the grants after that one. */
export interface ConversationSharesParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Absent on the first page. Later pages echo the cursor the previous page returned. */
  cursor?: ConversationSharesCursor
  /** How many grants this page may hold, from 1 through the published page size. A page of that many grants, each at its largest, fits one frame. */
  pageSize: number
}
/** One grant on a conversation. */
export interface ConversationShare {
  /** The paired device's credential, as credential.list names it. */
  credentialId: string
  /** What the grant allows: read; or read and comment; or read, comment and drive. */
  capabilities: ConversationCapability[]
  /** The constraints. Present exactly when capabilities includes drive. */
  policy?: ConversationToolPolicy
  /** The grant epoch. Moves each time the owner changes this grant's capabilities or constraints, and never otherwise. A drive turn is checked at the epoch it was admitted under. */
  policyRevision: number
  /** When the grant was made, Unix milliseconds. */
  grantedAtMs: number
}
/** One page of conversation.shares: this page's grants, oldest first, the conversation's catalogue revision when they were read, and a cursor when further grants remain. A later page belongs to the same read only when it names the same revision. A conversation holds at most 64 grants; one page holds at most its page size. */
export interface ConversationSharesResult {
  /** This page's grants, oldest first, at most the page size asked for. A conversation's grants may take more than one page. */
  items: ConversationShare[]
  /** The conversation's catalogue revision when this page was read, with its grants. A grant or a revoke moves it; a change of capabilities or constraints does not. A later page belongs to the same read only when it names this revision. */
  changeRevision: number
  /** Present when further grants remain after this page. Absent when this page finishes the list. The next page echoes it. */
  cursor?: ConversationSharesCursor
}
/** Result of conversation.share: whether it changed anything, and the grant's policy revision once it was made. */
export interface ConversationShareResult {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Whether the capabilities or constraints changed. False when the grant already held them. */
  applied: boolean
  /** The grant's policyRevision once this share was made: the new revision when applied, the one it kept when not. A retry answers the revision its first answer named. The expectedPolicyRevision a later change to this grant names. */
  policyRevision: number
}
/** Stable acknowledgement of one submitted input. */
export interface ConversationReceipt {
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Accepted or recovered submission state. */
  disposition: ConversationDisposition
}
/** Acknowledgement for a control action. */
export interface ConversationMutationResult {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Whether the requested change was applied or already acknowledged. */
  applied: boolean
}
/** Atomically replace the complete waiting order. All current waiting execution IDs must appear once; steering retains priority over ordinary queued input. */
export interface ConversationReorderParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Identity of this deliberate control action; an uncertain acknowledgement requires a fresh read, not replay. */
  requestId: string
  /** Exact desired full waiting order, including steering inputs. Running invocations are excluded. */
  executionIds: string[]
}
/** Applied or unchanged order, or a rejected stale queue/steering-priority conflict. Rejections leave the queue unchanged. */
export const ConversationReorderOutcome = {
  Applied: "applied",
  Unchanged: "unchanged",
  QueueChanged: "queue_changed",
  PriorityConflict: "priority_conflict",
} as const
export type ConversationReorderOutcome =
  (typeof ConversationReorderOutcome)[keyof typeof ConversationReorderOutcome]
/** Acknowledgement of one atomic queue reorder control. */
export interface ConversationReorderResult {
  /** Identity of this deliberate control action; an uncertain acknowledgement requires a fresh read, not replay. */
  requestId: string
  /** Whether the whole requested order was accepted. */
  outcome: ConversationReorderOutcome
}
/** The conversation's latest lease as its records fold. A lease is the recorded permission for one environment to run this conversation's agent with named limits; every run of the agent has one. A lease this gateway cannot read is shown as unreadable with nothing else claimed about it. */
export interface ConversationLease {
  /** live: the agent may run. ending: an end was decided and cleanup is awaited. ended: cleanup was confirmed. interrupted: cleanup was not confirmed within its deadline, so the agent's environment is not known to be released. refused: no lease was granted and nothing ran; refusal says why. unreadable: a record of this lease is of a kind this gateway cannot read. */
  state: "live" | "ending" | "ended" | "interrupted" | "refused" | "unreadable"
  /** Which issuance of this conversation's leases this is, counting refused ones; absent when not known. */
  revision?: number
  /** Where the agent runs. here: the gateway's own machine. ssh: a machine the gateway reaches over SSH, named by host, whose `nessa env serve` runs the agent's harness next to its files. peer: a paired environment, named by host, whose `nessa env serve` dials this gateway and runs the agent's harness next to its files. */
  environment?: "here" | "ssh" | "peer" | "boat"
  /** The machine the agent runs on: for ssh, the OpenSSH destination as config.json names it; for peer, `peer:` and the paired environment's device key in lowercase hex. Present exactly when environment is ssh or peer. */
  host?: string
  /** The sandbox granted, or for a refused lease the one asked for. harness_default: whatever the agent's harness encloses by default, configured by nothing Nessa sets, and nothing more. none: nothing encloses what runs beyond the account the environment runs as. */
  sandbox?: "harness_default" | "none"
  /** Why the lease is ending or ended: the first cause recorded. lost: the environment no longer held it, for example because the gateway started again or the connection to it was lost. idle: the agent sat idle with nobody following the conversation, past limits.agentIdleSeconds, and the gateway stopped it; the conversation stays open and the next message starts the agent again, resuming its provider session. */
  cause?: "stopped" | "closed" | "revoked" | "expired" | "lost" | "idle"
  /** What the environment reported releasing, once it has: confirmed, released cooperatively; forced, released by forced termination; not_held, the environment holds nothing for the lease, though whether something started under it before the environment last started outlived it is not known. On an interrupted lease this is the evidence that arrived after its deadline. */
  cleanup?: "confirmed" | "forced" | "not_held"
  /** Why the lease was refused. sandbox_unavailable: the sandbox asked for is not one both the agent's binding and the environment can enforce. environment_unreachable: the environment could not be reached, or did not answer in time. environment_version_mismatch: the copy of this build the environment holds, where the gateway installs it, speaks another lease protocol, so nothing was sent to it. environment_busy: the environment is serving another connection, or the organization's paired environments already run as many agent leases or commands as config.json limits.organizationLeases and limits.organizationCommands allow. agent_unavailable: the environment cannot run this conversation's agent. environment_platform_unsupported: the environment had no copy of this build, and runs on a system or processor this build has no copy for (its own executable, or the release build of it for that platform), so nothing was installed or sent. environment_install_failed: the environment had no copy of this build, and installing one failed: the release build for its platform could not be downloaded or was not the one pinned, or what arrived did not match what was sent, did not run there, or could not be stored; nothing was sent. */
  refusal?:
    | "sandbox_unavailable"
    | "environment_unreachable"
    | "environment_version_mismatch"
    | "environment_busy"
    | "agent_unavailable"
    | "environment_platform_unsupported"
    | "environment_install_failed"
  /** Events that arrived after the lease ended and were dropped rather than applied. */
  droppedEvents: number
  /** Commands the agent started under this lease that have not yet ended; absent when none. Present only on a live or ending lease. */
  commands?: number
}
/** Runtime configuration selected by the gateway composition for this conversation. */
export interface ConversationRuntime {
  /** Configured model identifier. */
  model: string
  /** Configured agent provider name. */
  provider: string
  /** Working directory on the gateway host, not the client filesystem. */
  workspace: string
  /** Conversation agent identity. */
  agent: string
  /** Catalog display name. */
  modelName: string
  /** Catalog context-window ceiling. */
  contextWindowTokens: number
  /** Catalog reasoning capability. */
  reasoning: boolean
}
/** Which local decision a notice records. review is a declined review. hook is a permission-review hook that stopped the turn or closed the session. deadline is a deadline that settled its review. The three may share one decimal id. */
export const ConversationNoticeKind = {
  Review: "review",
  Hook: "hook",
  Deadline: "deadline",
} as const
export type ConversationNoticeKind =
  (typeof ConversationNoticeKind)[keyof typeof ConversationNoticeKind]
/** One ordered observation fragment in a bounded execution projection; offsets may have gaps. */
export interface ConversationPart {
  /** Zero-based SDK observation offset within the owning execution, used to preserve order. */
  offset: number
  /** Text, exposed thought content, a tool call, or a Nessa-owned runtime notice. A tool call is one part however many updates it has, at its first update's offset; its current state is its entry in tools. */
  kind: "text" | "thought" | "tool" | "local_notice"
  /** Exact text for text, thought, or local notice observations; empty for a tool call. Adjacent streamed chunks of one kind are joined into one part, so its only bound is the frame's. */
  text: string
  /** The tool call's identity, which names its entry in tools; empty otherwise. */
  toolId: string
  /** Decimal identity of a local notice; empty otherwise. A review notice uses the declined-review id. A hook notice and a deadline notice use the permission id. Those identities are independent, so one decimal may name each of them; noticeKind says which decision it is. */
  noticeId: string
  /** Which local decision this notice is. Absent on every part that is not a local notice. */
  noticeKind?: ConversationNoticeKind
  /** Opaque provider message identity; only fragments with the same identity may be combined. */
  messageId?: string
}
/** What a review asks the person to allow: running a tool (tool) — the agent's tool call, or a tool an MCP App asked to call on its own server — or an MCP App sending one message in the conversation as them (message; mcp.sendMessage, where toolName is the app's own tool and argumentsJson is {"text": …}, the message exactly as it would be sent). The agent's reviews are always tool. */
export const ConversationPermissionAsk = { Tool: "tool", Message: "message" } as const
export type ConversationPermissionAsk =
  (typeof ConversationPermissionAsk)[keyof typeof ConversationPermissionAsk]
/** Who asked for a review: the conversation's agent (harness), or an MCP App (app), asking to call a tool of its own server or to send a message as the person (ask). The agent's approval mode never applies to an app's review. */
export const ConversationPermissionOriginKind = {
  Harness: "harness",
  App: "app",
} as const
export type ConversationPermissionOriginKind =
  (typeof ConversationPermissionOriginKind)[keyof typeof ConversationPermissionOriginKind]
/** Who asked for this review. For harness, the review's executionId and toolId are the agent's tool call being reviewed. For app, they are the app's identity — the tool call whose UI it is, which has normally finished — and server and tool name the tool the app asked to call, or for a message (ask: message) the app's own server and tool (required for app, absent for harness). A harness review is shown only while its execution runs; an app review while the app waits on it, whatever its tool call's state. Answer either kind with conversation.answer or conversation.cancel. */
export interface ConversationPermissionOrigin {
  /** Who asked. */
  kind: ConversationPermissionOriginKind
  /** For app: the app's server, on which the tool would be called. */
  server?: string
  /** For app: the tool the app asked to call, the review's toolName. */
  tool?: string
}
/** An MCP App, by the tool call whose UI it is, in its conversation. The gateway checks it is an MCP call of the server the request names, and that its result carried a resourceUri. It is not authenticated beyond the caller's credential: the call is recorded as the app's, on the person's behalf. instanceId names which mount of it is asking. */
export interface McpAppReference {
  /** The execution the app's tool call belongs to. */
  executionId: string
  /** The app's tool call. */
  toolId: string
  /** The host's own UUID for this mount of the app. One tool call can be mounted more than once (inline, in a pane) and again after it was torn down; reviews and tickets are kept per mount, and mcp.releaseApp releases only that one. Policy and audit name the app by executionId and toolId. */
  instanceId: string
}
/** The host tore an app's mount down (mcp.releaseApp): every review that mount has open is withdrawn and its waiting call answered mcp_cancelled, and every resource ticket issued to it is released. Releasing a mount with nothing open succeeds too. Answered with ConversationMutationResult. It travels on the control lane, never the app lane, so held calls can never stop an app being released. */
export interface McpReleaseAppParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The mount torn down. */
  app: McpAppReference
}
/** An MCP App sends a message into its conversation (mcp.sendMessage, MCP Apps ui/message): the person's turn, written by the app on their behalf and shown in the transcript as the app's (ConversationMessage.app). Every message waits for the person's approval in the conversation's permissions, origin {kind: app}, answered with conversation.answer or conversation.cancel whatever the approval mode. Refused turn_running while a turn runs or input waits, so it is never queued behind the person's own; any other refusal of the message is its own conversation code. The same requestId again, from the same mount, is the same turn: one the agent has already is not asked again, and the agent settles it, the same text answering the first delivery and other text submission_conflict; one sent again while the first is still in review or being sent is refused temporarily_unavailable. It travels on the app lane. */
export interface McpSendMessageParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The app sending it. */
  app: McpAppReference
  /** The MCP server's configured name: the app's own server. Any other is refused mcp_server_mismatch. */
  server: string
  /** The message, as text: at most 8192 UTF-8 bytes, what conversation.send takes (invalid_request past it, refused before anything is recorded), and not empty (invalid_request, refused before anything is recorded). Blank text, whitespace only, is invalid_request too, refused on record by the conversation. Past the conversation's own input bound, which is never larger, it is refused mcp_request_too_large. It is shown whole in its review, which must fit the 16 000 bytes an app's review may take of the view, encoded; text heavy in quotes or control characters can be past that while within 8192 bytes (mcp_request_too_large, and no review is opened). */
  text: string
}
/** The conversation's agent took the message. */
export interface McpSendMessageResult {
  /** The turn the message became, as the transcript names it. */
  executionId: string
}
/** An MCP App gives the model context (mcp.updateModelContext, MCP Apps ui/update-model-context), in place of what this mount gave before; an update with neither text nor structuredContentJson, or with only an empty text, clears it. It is held for the mount until the next message admitted into the conversation while nothing runs and no input waits, the person's or an app's, carries it ahead of what the message says; it is not shown in the transcript. That message takes it as it is read, and it is held no longer. If the message is then refused, it is lost, recorded as dropped not_sent; if its turn fails, it is lost; either way the app may give it again. A message queued behind a running turn, or steered into one, carries none and leaves it held. A release of the mount (mcp.releaseApp) or the end of the conversation's opening drops it unsent while it is still held. A conversation's updates are taken one at a time, each recorded before it is held. Each part takes at most 8192 UTF-8 bytes (invalid_request past it, refused before anything is recorded), and text and structured content together at most 8192 (mcp_request_too_large past it); at most 4 mounts of a conversation hold a context at once (temporarily_unavailable for another). Answered with ConversationMutationResult. It travels on the app lane. */
export interface McpUpdateModelContextParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The app giving it. */
  app: McpAppReference
  /** The MCP server's configured name: the app's own server. Any other is refused mcp_server_mismatch. */
  server: string
  /** The context as text: at most 8192 UTF-8 bytes (invalid_request past it). Empty is none. */
  text?: string
  /** The context's structured content: one JSON object, encoded (invalid_request if it is not one, or past 8192 UTF-8 bytes), held exactly as given. Absent is none. */
  structuredContentJson?: string
}
/** An MCP App calls a tool of its own server (mcp.callTool). Allowed only for a tool its conversation's own session last listed with visibility including app. A tool that is destructive — readOnlyHint is not true and destructiveHint is not false, so a tool with no annotations is — first waits for the person's approval in the conversation's permissions, whatever the approval mode; the call is answered when they answer, when the review expires (x-mcpAppCallTiming.reviewDeadlineMs), or when it is withdrawn. App calls travel on a lane of their own, 4 at once per socket, at most 3 of them from one mount, so one app's waiting reviews cannot take the lane; past either they are refused temporarily_unavailable. */
export interface McpCallToolParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The app asking. */
  app: McpAppReference
  /** The MCP server's configured name: the app's own server. A call naming any other is refused mcp_server_mismatch. */
  server: string
  /** The tool to call on that server. */
  tool: string
  /** The tool's arguments: one JSON object, encoded, at most 32 KiB (mcp_request_too_large past it, invalid_request if it is not an object), sent as parsed — re-encoded, a duplicate key's last value kept — and shown so in a destructive tool's review, which may take at most 16 000 bytes of the view (mcp_request_too_large past it). Absent is none. */
  argumentsJson?: string
}
/** The tool's answer. */
export interface McpCallToolResult {
  /** The MCP CallToolResult the server answered — content, structuredContent, isError, _meta — re-encoded as one JSON object, at most 56 KiB measured as the JSON string this field is (past it, the call is refused mcp_result_too_large instead). isError true is a result for the app, not a refusal. */
  resultJson: string
}
/** An MCP App reads a resource of its own server (mcp.readResource). The gateway reads it once, holds those bytes, and answers what they are and a ticket that serves exactly them over HTTP (GET /mcp-resources, the ticket in the x-nessa-resource-ticket header): the bytes never travel on the socket. App calls share their own lane, as mcp.callTool's. */
export interface McpReadResourceParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The app asking. */
  app: McpAppReference
  /** The MCP server's configured name: the app's own server. A call naming any other is refused mcp_server_mismatch. */
  server: string
  /** The resource's URI on that server. */
  uri: string
}
/** The CSP the app asked for (_meta.ui.csp): origins only, each list at most 64 of at most 512 bytes. No network when empty. */
export interface McpUiCsp {
  /** connect-src origins. */
  connectDomains: string[]
  /** Origins for scripts, styles, images, fonts and media. */
  resourceDomains: string[]
  /** frame-src origins. */
  frameDomains: string[]
  /** base-uri origins. */
  baseUriDomains: string[]
}
/** What the app asked of the host (_meta.ui.permissions). Each is true only when asked for as {} or true; nothing else is ever granted. */
export interface McpUiPermissions {
  /** Asked for camera. */
  camera: boolean
  /** Asked for microphone. */
  microphone: boolean
  /** Asked for geolocation. */
  geolocation: boolean
  /** Asked for clipboardWrite. */
  clipboardWrite: boolean
}
/** What the gateway read, and the ticket for its bytes. The host fetches GET /mcp-resources with the ticket in the x-nessa-resource-ticket header — never in a URL — checks the bytes' SHA-256 against sha256 before rendering, and treats a mismatch as a failure to load. */
export interface McpReadResourceResult {
  /** The resource read. */
  uri: string
  /** An MCP App's HTML; any other resource is refused mcp_app_unknown. */
  mimeType: "text/html;profile=mcp-app"
  /** The bytes' length: at most 4 MiB. */
  size: number
  /** Lowercase hex SHA-256 of exactly the bytes the ticket serves. */
  sha256: string
  /** Single use, 256 random bits (base64url), valid for expiresInMs, bound to the conversation and the app. It is the whole authority to fetch the bytes, as an upload ticket is: keep it out of URLs and logs. */
  ticket: string
  /** How long the ticket can be redeemed. */
  expiresInMs: 60000
  /** The CSP the app asked for. */
  csp: McpUiCsp
  /** What the app asked of the host. */
  permissions: McpUiPermissions
  /** The dedicated origin the app asked for, when it did. */
  domain?: string
  /** Whether the app asked for a border, when it said. */
  prefersBorder?: boolean
}
/** The server's own JSON-RPC error, attached to a refusal coded mcp_remote_error, or mcp_server_remote_error from mcpServers.inspect. */
export interface McpRemoteErrorDetails {
  /** The JSON-RPC error code. */
  code: number
  /** The server's message, at most 512 characters, control characters as spaces. */
  message: string
}
/** How a configured MCP server is reached. stdio: the gateway starts it as a local process and speaks MCP over its standard input and output. remote: the gateway connects to an HTTP endpoint. A remote entry has id and url, and no command, args, or env. */
export const McpServerKind = { Stdio: "stdio", Remote: "remote" } as const
export type McpServerKind = (typeof McpServerKind)[keyof typeof McpServerKind]
/** One server as config.json stores it now, which mcpServers.list reads: the stored file, not the live set. A hand edit to the file is listed at once and reaches the live set, and new conversations, at the next save or remove or the next start. Variable values never leave the gateway; only their names are listed. A stdio entry includes command, args and envNames. A remote entry includes id and url, and none of those three. */
export interface McpServerListEntry {
  /** How the server is reached. */
  kind: McpServerKind
  /** The server's name, unique among the configured servers. */
  name: string
  /** The absolute path of the executable the gateway starts. */
  command?: string
  /** The arguments it is started with, in order. */
  args?: string[]
  /** The names of the variables it is given over the gateway's own, sorted by name. Never their values. */
  envNames?: string[]
  /** Whether it is stored turned on. A server saved on is given to new conversations from that save; one turned on by hand in the file, from the next save or remove or the next start. A server turned off stays stored, and can be inspected. */
  enabled: boolean
  /** Nessa's own server: listed, never saved or removed through these methods. */
  managed: boolean
  /** The remote server's durable id. Present exactly when kind is remote. A rename does not change it. A save cannot choose it. */
  id?: string
  /** The remote endpoint. Present exactly when kind is remote. */
  url?: string
  /** Redacted authorization facts for a remote server. Absent for stdio, and for a remote with no record yet. Never a token. */
  authorization?: McpServerAuthorization
}
/** Where a remote server's authorization stands. unauthenticated: the endpoint accepted a probe with no credentials. consent_needed: a person must authorize, or discovery has not finished. pending_consent: a consent URL is outstanding. ready: a usable token is stored. scope_required: the last call was refused for scope and was not retried. authorization_incomplete: an exchange or refresh was not settled. revoking: revoke is in progress. revocation_incomplete: local cleanup or its evidence did not finish. */
export const McpAuthorizationPhase = {
  Unauthenticated: "unauthenticated",
  ConsentNeeded: "consent_needed",
  PendingConsent: "pending_consent",
  Ready: "ready",
  ScopeRequired: "scope_required",
  AuthorizationIncomplete: "authorization_incomplete",
  Revoking: "revoking",
  RevocationIncomplete: "revocation_incomplete",
} as const
export type McpAuthorizationPhase =
  (typeof McpAuthorizationPhase)[keyof typeof McpAuthorizationPhase]
/** What the authorization server did with a revocation. acknowledged: its revocation endpoint answered success. unsupported: it advertised no revocation endpoint. unconfirmed: the call was sent and the answer was not retained, or it failed. */
export const McpRemoteObservation = {
  Acknowledged: "acknowledged",
  Unsupported: "unsupported",
  Unconfirmed: "unconfirmed",
} as const
export type McpRemoteObservation =
  (typeof McpRemoteObservation)[keyof typeof McpRemoteObservation]
/** Redacted authorization facts mcpServers.list may attach to a remote server. No token, refresh secret, or authorization code is included. */
export interface McpServerAuthorization {
  /** Where authorization stands. */
  phase: McpAuthorizationPhase
  /** The token generation. A refresh that published a replacement increments it. */
  generation: number
  /** Whether the stored access token is past its expiry or was marked unavailable. */
  tokenExpired: boolean
  /** Whether a refresh is in flight, or the last refresh was not settled. */
  refreshFailing: boolean
  /** Whether the last call was refused for scope. The call was not retried. */
  scopeRequired: boolean
  /** The last revocation's remote observation, when a revoke has been asked. */
  remoteObservation?: McpRemoteObservation
  /** The domain-set digest a person acknowledged for this server, when one has been. */
  domainsDigest?: string
}
/** Wire input for mcpServers.authorize: begin consent for the remote server id at the listed revision. A stdio server cannot be authorized. Status and list reads do not open a browser; this method returns a consent URL when one is required. */
export interface McpServersAuthorizeParams {
  /** The revision the caller last listed. */
  revision: string
  /** The remote server's durable id. */
  id: string
}
/** Result of mcpServers.authorize. No token is included. pending_consent carries the URL the host opens and the deadline of that attempt. ready means a token is already usable. not_required means the endpoint accepted a probe with no credentials. */
export interface McpServersAuthorizeResult {
  /** What authorize settled to. */
  status: "not_required" | "pending_consent" | "ready"
  /** The consent attempt, present for pending_consent. */
  attemptId?: string
  /** The authorization URL, present for pending_consent. It carries no code verifier. */
  consentUrl?: string
  /** When the consent attempt expires, as milliseconds since the Unix epoch. Present for pending_consent. */
  deadlineMs?: number
  /** The token generation, present for ready. */
  generation?: number
}
/** Wire input for mcpServers.revoke: fence the remote server id at the listed revision. Local sessions of that server are closed. The result keeps drain, secret deletion, remote observation and evidence apart; an incomplete revoke is not reported as settled. */
export interface McpServersRevokeParams {
  /** The revision the caller last listed. */
  revision: string
  /** The remote server's durable id. */
  id: string
}
/** Result of mcpServers.revoke. settled is true only after the local drain, secret deletion, remote observation and required evidence all finished. An incomplete result stays incomplete. */
export interface McpServersRevokeResult {
  /** Whether revoke finished. */
  settled: boolean
  /** Whether local sessions of this server were closed. */
  localDrained: boolean
  /** Whether the private token is gone. */
  secretDeleted: boolean
  /** What the authorization server did, when that was observed. */
  remoteObservation?: McpRemoteObservation
  /** Whether the outcome record was acknowledged. */
  evidenceAcknowledged: boolean
}
/** Attached to a refusal coded mcp_servers_authorization_held. The configuration file was written. The live set was not replaced, because the previous token binding could not be fenced. */
export interface McpServersAuthorizationHeldDetails {
  /** Whether the configuration file was written. True: it was, and the live set still has the previous URL. */
  applied: boolean
}
/** Result of mcpServers.list: the stored servers and the revision a save or remove must name. */
export interface McpServersListResult {
  /** A digest of the stored server list, keyed with a secret this gateway process mints at start: compare for equality only, and list again after a restart, since the same list has another revision then. A save or remove naming any other revision is refused mcp_servers_revision_conflict. */
  revision: string
  /** The stored servers in stored order, then the managed one. */
  servers: McpServerListEntry[]
}
/** One variable a saved server is given. */
export interface McpServerEnvEntry {
  /** The variable's name: ASCII letters, digits and _, 1 to x-mcpServerRules.environmentNameMaxBytes bytes, not starting with a digit. NESSA_MCP_SESSION is reserved. */
  name: string
  /** Its value, or null to keep the value stored for this name on the server being saved, only when the save launches that server as stored apart from the kept values: the same command and args, and the same variable names, each other one null or given its stored value. A save that changes anything else (the command, an argument, a variable added, left out or given another value) must give every value again. Required: an entry without value is refused invalid_request, so leaving it out never keeps a value by accident. A null for a name with no stored value is refused mcp_servers_invalid (environment_value_missing). */
  value: string | null
}
/** A server as mcpServers.save stores it. kind stdio requires command, args and env, and refuses url. kind remote requires url, and refuses command, args and env. A remote save cannot choose the stored id. */
export interface McpServerInput {
  /** How the server is reached. */
  kind: McpServerKind
  /** Its name: ASCII letters, digits, - and _, 1 to x-mcpServerRules.nameMaxBytes bytes, without __ and not starting or ending with _. nessa is reserved. */
  name: string
  /** The absolute path of its executable. */
  command?: string
  /** Its arguments, in order: at most x-mcpServerRules.maxArgs, each at most x-mcpServerRules.argMaxBytes bytes. Never put credentials here; use env. */
  args?: string[]
  /** Every variable it is given over the gateway's own, each name once; they are stored, and listed, sorted by name whatever order they are given in. A stored variable left out is removed. */
  env?: McpServerEnvEntry[]
  /** Whether new conversations are given it. */
  enabled: boolean
  /** The remote endpoint. Required when kind is remote, and refused when kind is stdio. The gateway checks it; this field does not restate that rule. */
  url?: string
}
/** Wire input for mcpServers.save: store a server, adding it or replacing the one under its name, or renaming the one under previousName, in one write. */
export interface McpServersSaveParams {
  /** The revision the caller last listed. */
  revision: string
  /** The name the server is stored under now, when it is being renamed. Unknown: mcp_servers_not_found. */
  previousName?: string
  /** The server as it should be stored. */
  server: McpServerInput
}
/** Wire input for mcpServers.remove: take a stored server out of the configuration. */
export interface McpServersRemoveParams {
  /** The revision the caller last listed. */
  revision: string
  /** The stored server's name. Unknown: mcp_servers_not_found. */
  name: string
}
/** Result of mcpServers.save and mcpServers.remove: the published configuration's revision. New conversations get the new server set; running ones keep theirs. */
export interface McpServersWriteResult {
  /** The stored server list's revision now, for this gateway process. */
  revision: string
  /** Whether new conversations get the stored list as now written. False when the gateway is stopping (the next start reads the file), or when a remove left a list edited by hand still past a bound (more servers than x-mcpServerRules.maxServers, or one that breaks a rule): the removed server is out of the live set all the same, and the rest stay as they were until a change brings the list within its bounds. */
  live: boolean
}
/** Why a saved server, or a stored one inspected, is invalid. The numbers are x-mcpServerRules, which the gateway's own rules publish. too_many: more than maxServers servers, the managed one included. duplicate_name: another server has the name. name: not 1 to nameMaxBytes bytes of ASCII letters, digits, - and _, holds __, or starts or ends with _. command: not an absolute UTF-8 path. arguments: more than maxArgs, or one over argMaxBytes bytes or holding NUL. environment_name: a variable name that is not ASCII letters, digits and _ (1 to environmentNameMaxBytes bytes, not starting with a digit). reserved_environment_name: NESSA_MCP_SESSION. environment_value: a value holding NUL. environment_value_missing: a null value for a name with no stored value, or in a save that changes anything else the server is launched with. environment_name_repeated: a variable given twice, which is said before a value missing. url: a remote endpoint that is not https, and not http on an explicit loopback host, or that carries userinfo or a fragment. duplicate_server_id: two remote servers are stored with one id. */
export const McpServerProblemCode = {
  TooMany: "too_many",
  DuplicateName: "duplicate_name",
  Name: "name",
  Command: "command",
  Arguments: "arguments",
  EnvironmentName: "environment_name",
  ReservedEnvironmentName: "reserved_environment_name",
  EnvironmentValue: "environment_value",
  EnvironmentValueMissing: "environment_value_missing",
  EnvironmentNameRepeated: "environment_name_repeated",
  Url: "url",
  DuplicateServerId: "duplicate_server_id",
} as const
export type McpServerProblemCode =
  (typeof McpServerProblemCode)[keyof typeof McpServerProblemCode]
/** Attached to a refusal coded mcp_servers_invalid. */
export interface McpServersInvalidDetails {
  /** What is wrong. */
  problem: McpServerProblemCode
  /** The server the problem is about, as it is stored or was asked to be saved: present for every problem but too_many, so an entry added to config.json by hand is named. */
  server?: string
  /** The variable the problem is about, for environment_name, reserved_environment_name, environment_value, environment_value_missing and environment_name_repeated; for environment_name, any byte of it that is not UTF-8 replaced. Never a value. */
  name?: string
}
/** Attached to a refusal coded mcp_servers_revision_conflict. */
export interface McpServersRevisionConflictDetails {
  /** The stored revision now; list again before retrying. */
  revision: string
}
/** Attached to a refusal coded audit_unavailable from an mcpServers method. Nothing is rolled back; mcpServers.list shows where things stand. */
export interface McpServersAuditUnavailableDetails {
  /** For mcpServers.save and mcpServers.remove: whether the change was published all the same; the live set may not have been replaced if the gateway is stopping. For mcpServers.inspect: whether the server was started — or its launch had begun, when the work failed unexpectedly after that. */
  applied: boolean
  /** What the request would have been answered had its record been written: the refusal or failure that stopped it, or mcp_servers_storage_unavailable for a change published but not made durable. Absent when nothing stopped it, or when the first record could not be written and nothing was done. Never audit_unavailable. */
  code?: McpServersErrorCode
}
/** Attached to a refusal coded mcp_servers_config_too_large that answers mcpServers.list: the stored list would not fit one frame (an entry added to config.json by hand). mcpServers.remove by name, naming this revision, still works, and is how the list is made to fit again. */
export interface McpServersConfigTooLargeDetails {
  /** The stored revision, as mcpServers.list would have answered it. */
  revision: string
}
/** Attached to a refusal coded mcp_servers_storage_unavailable. mcpServers.list shows where things stand. */
export interface McpServersStorageUnavailableDetails {
  /** true: the change was published — config.json replaced, and the live set followed as far as it could (replaced, withdrawn, or kept during shutdown or when the list is past a bound); list again to see the current state — but its directory could not be synced, so a crash could still lose it; its outcome record says durable false. false: nothing was written, applied or started. */
  applied: boolean
}
/** Wire input for mcpServers.inspect: start the stored server under name once, outside any conversation, list what it offers, then stop it. A server turned off can be inspected; a server not yet saved cannot. At most x-mcpServerInspect.maxConcurrent run at once, each within x-mcpServerInspect.deadlineMs. */
export interface McpServersInspectParams {
  /** The stored server's name. Unknown: mcp_servers_not_found; nessa: mcp_servers_reserved_name. */
  name: string
}
/** Which bound left an inspection incomplete. tools: the server listed more than x-mcpServerInspect.maxToolPages pages, or more tools than the gateway reads from one server; the rest are not listed. ui: more distinct UI resources than x-mcpServerInspect.maxUiReads; tools past it are listed without ui. bytes: the answer would pass 64 KiB; tools were dropped from the end until it fits. stopping: the gateway began to stop after the server was started; the reading was dropped and the server's process group killed, and tools is empty. */
export const McpServersInspectCut = {
  Tools: "tools",
  Ui: "ui",
  Bytes: "bytes",
  Stopping: "stopping",
} as const
export type McpServersInspectCut =
  (typeof McpServersInspectCut)[keyof typeof McpServersInspectCut]
/** A tool's MCP App as the server declares it: the resource, and the CSP and permissions it asks for, read as mcp.readResource reads them. */
export interface McpInspectedUi {
  /** The ui:// resource the tool declares. */
  uri: string
  /** The CSP the app asks for. */
  csp: McpUiCsp
  /** What the app asks of the host. */
  permissions: McpUiPermissions
}
/** One tool a server lists, as mcpServers.inspect reports it. */
export interface McpInspectedTool {
  /** The tool's name on the server. */
  name: string
  /** annotations.readOnlyHint, when the server gives it as a boolean. */
  readOnlyHint?: boolean
  /** annotations.destructiveHint, when the server gives it as a boolean. */
  destructiveHint?: boolean
  /** Its MCP App, when it declares one and it was read. */
  ui?: McpInspectedUi
}
/** Result of mcpServers.inspect: the tools the server listed, with each MCP App's CSP and permissions. The server has been stopped and its process group killed before this is answered. */
export interface McpServersInspectResult {
  /** Whether every tool and UI the server offered is here. */
  complete: boolean
  /** Which bound left it incomplete; present exactly when complete is false. */
  cut?: McpServersInspectCut
  /** The tools, in the server's order. */
  tools: McpInspectedTool[]
}
/** Why the gateway refused an mcpServers method it dispatched. mcp_servers_not_configured: this gateway holds no live MCP server set to manage (not Unix, no agents configured, or MCP off this run because its relay socket could not be bound, a path was not UTF-8, or no key for its configuration digests could be drawn). mcp_servers_invalid: the saved server or the resulting set breaks a rule; or, for mcpServers.inspect, the stored server does (an entry added to config.json by hand), and it was not started (details: McpServersInvalidDetails). mcp_servers_reserved_name: the request names nessa, Nessa's own server. mcp_servers_not_found: no server is stored under the name. mcp_servers_revision_conflict: the stored list changed since the caller's revision (details: McpServersRevisionConflictDetails). mcp_servers_busy: another change held config.json's lock too long, and nothing was written; or, for mcpServers.inspect, x-mcpServerInspect.maxConcurrent inspections are running already, and nothing was started. mcp_servers_config_invalid: config.json does not parse, as it is or as it would be written (a stored server name past x-mcpServerRules.nameMaxBytes among it, so every request naming a stored server fits one frame). It is never repaired. mcp_servers_config_too_large: the result would pass 64 KiB as written, which is pretty-printed or, when only that fits, compact; or, for mcpServers.save, the resulting mcpServers.list answer would not fit one frame for the longest request id, and nothing was written (a remove is never refused for that); or, for mcpServers.list, the stored list would not fit one frame (details: McpServersConfigTooLargeDetails), or config.json itself passes 64 KiB (no details). mcp_servers_storage_unavailable (details: McpServersStorageUnavailableDetails): config.json could not be read, locked or published, or the work failed unexpectedly before it published or before the inspected server's launch began — its outcome recorded failed, panicked — and the stored file and the live set are unchanged (applied false); or the change was published, and the live set followed as far as it could (replaced, withdrawn, or kept during shutdown or when the list is past a bound), but config.json's directory could not be synced, so the change may not survive a crash (applied true); list again to see the current state. mcp_servers_stopping: the gateway is stopping, and nothing was started or written: the request came after shutdown began, and nothing was recorded; or, for mcpServers.inspect, shutdown began before the server was started, or the MCP client refused to start it, as the inspection's outcome record says (stopping, started false). Changes admitted before shutdown finish, and are recorded, before the gateway stops its MCP servers; an inspection already started is cut instead (McpServersInspectCut stopping). audit_unavailable: a record of the change or inspection could not be made durable, or the work failed unexpectedly after the change was published or the server's launch began, so no outcome was recorded (details: McpServersAuditUnavailableDetails). The mcp_server_ codes answer mcpServers.inspect, whose server was stopped on each: mcp_server_start_failed, its process could not be launched (a missing command among them); mcp_server_timed_out, it did not finish within x-mcpServerInspect.deadlineMs, or did not answer a request in time; mcp_server_gone, it ended before it answered; mcp_server_malformed, it answered something that is not MCP, or a UI resource that is not an MCP App within its bounds; mcp_server_remote_error, it answered a request with a JSON-RPC error (details: McpRemoteErrorDetails). mcp_server_unreachable, a remote endpoint could not be reached and nothing was retained. mcp_server_unauthorized, the endpoint refused the caller; the answer carries no token. mcp_server_insufficient_scope, the caller's scope was not enough and the call was not retried. mcp_server_session_collision, another opening already holds that upstream session id, and this inspection did not delete it. mcp_servers_authorization_held (details: McpServersAuthorizationHeldDetails): a save or remove wrote config.json, and the live set was not replaced because the previous remote token could not be fenced; applied is true. mcp_servers_store_unavailable: this platform has no private token writer, so authorize did not start. mcp_servers_registration_unsupported: the authorization server does not offer dynamic client registration. mcp_servers_discovery_failed: discovery, the callback, or the token exchange was refused, and no token was stored. mcp_servers_authorization_incomplete: an exchange or refresh was sent and its result was not retained, so the token is fenced until authorize is repeated. */
export const McpServersErrorCode = {
  McpServersNotConfigured: "mcp_servers_not_configured",
  McpServersInvalid: "mcp_servers_invalid",
  McpServersReservedName: "mcp_servers_reserved_name",
  McpServersNotFound: "mcp_servers_not_found",
  McpServersRevisionConflict: "mcp_servers_revision_conflict",
  McpServersBusy: "mcp_servers_busy",
  McpServersConfigInvalid: "mcp_servers_config_invalid",
  McpServersConfigTooLarge: "mcp_servers_config_too_large",
  McpServersStorageUnavailable: "mcp_servers_storage_unavailable",
  AuditUnavailable: "audit_unavailable",
  McpServersStopping: "mcp_servers_stopping",
  McpServerStartFailed: "mcp_server_start_failed",
  McpServerTimedOut: "mcp_server_timed_out",
  McpServerGone: "mcp_server_gone",
  McpServerMalformed: "mcp_server_malformed",
  McpServerRemoteError: "mcp_server_remote_error",
  McpServerUnreachable: "mcp_server_unreachable",
  McpServerUnauthorized: "mcp_server_unauthorized",
  McpServerInsufficientScope: "mcp_server_insufficient_scope",
  McpServerSessionCollision: "mcp_server_session_collision",
  McpServersAuthorizationHeld: "mcp_servers_authorization_held",
  McpServersStoreUnavailable: "mcp_servers_store_unavailable",
  McpServersRegistrationUnsupported: "mcp_servers_registration_unsupported",
  McpServersDiscoveryFailed: "mcp_servers_discovery_failed",
  McpServersAuthorizationIncomplete: "mcp_servers_authorization_incomplete",
} as const
export type McpServersErrorCode =
  (typeof McpServersErrorCode)[keyof typeof McpServersErrorCode]
/** Typed rejection code carried by a conversation command the gateway dispatched and refused. Branch on these instead of message text. These are not every code a conversation request can receive: access and routing failures are answered by the session before a conversation command is dispatched, and carry their own codes. agent_startup_deadline means the agent was still starting when its budget expired, so nothing reached the provider and the same command is safe to repeat; it normally succeeds once the runtime is warm, but a launch whose process could not be confirmed stopped keeps that conversation blocked. invalid_request and agent_not_configured reject the command until their cause is addressed. agent_not_configured, agent_unsupported and conversations_not_configured are three different situations and only one of them is fixed by configuring an agent: the gateway runs no conversations at all, it names no runtime under the agent this conversation asked for, or no build here can open that conversation's agent. sandbox_unavailable means no lease could be granted to run the agent because the sandbox it asks for is not one both its agent binding and the place it would run can enforce; nothing ran, the refusal is in the conversation's records, and repeating the command fails the same way until that configuration changes. environment_not_configured is a create naming an SSH destination config.json does not name, or a conversation created on one it no longer names; nothing ran. environment_unavailable is an SSH environment that could not be reached, is serving another connection, or cannot run this conversation's agent: no lease was granted, the refusal is in the conversation's records, and the same command may succeed once the environment is reachable. environment_version_mismatch is an SSH environment whose copy of this build, where the gateway installs it, says it speaks another lease protocol: nothing was sent to it. environment_platform_unsupported is an SSH environment with no copy of this build that runs on a system or processor this build has no copy for: the gateway installs its own executable, or on another platform the release build of it pinned for that platform, which a development build has none of; nothing was installed or sent. Run the conversation on a host like the gateway's, or from a release. environment_install_failed is an SSH environment with no copy of this build where installing one failed: the release build for its platform could not be downloaded or was not the one pinned, or what arrived did not match what was sent, did not run there, or could not be stored, and nothing was sent; the gateway's environment audit says which, and the same command tries the install again. linked_file_unreachable is a message linking a file by its path on this machine, sent to a conversation that runs on an SSH host: the host cannot read this machine's files and would read whatever it has at that path, so nothing was recorded or sent; send the message without it, or attach an image, whose bytes travel with it. The image codes answer `attachment.begin`, a message naming uploads, and `artifact.read`: image_input_unsupported is a model that takes no images, so no ticket and no message with one will ever be taken; attachment_not_found is an image or artifact this conversation does not hold — never uploaded or published into it, expired, or released when it closed — which also ends an artifact.read; attachment_unavailable is one it holds but could not read; attachment_capacity is no room for another upload right now; attachment_storage_unavailable is the gateway unable to keep the bytes. attachment.write answers four codes of its own, and each refusal of a piece ends that upload: upload_ticket_invalid is a ticket that cannot be used (never issued, expired, or already spent); upload_failed is an upload that stopped short of being kept (a piece out of order, bytes that did not match the declared size or digest, pieces that stopped past the upload deadline, or a conversation that let go of its files meanwhile), and beginning again is the whole remedy; unsupported_image is bytes that are not an image the gateway can read; image_too_large is an image past what the model takes even once reduced. attachment_cleanup_unavailable is a close that did happen, whose release of this conversation's uploads did not, and is the one image code that is not a refusal of the command. conversation_deleted refuses every command its owner sends on a conversation somebody deleted, except deleting it again; anyone else is told conversation_not_found. Its identity is never reused, so a surface still holding it should let it go. conversation_erasure_incomplete is a delete that did happen — the conversation is gone and every command on it is refused — whose erasure of stored data did not finish; repeating the delete, and each gateway start, tries again, but an agent that keeps refusing to delete its own session, or a damaged history, needs the operator. The mcp_ codes refuse an MCP App's request (mcp.callTool, mcp.readResource, mcp.sendMessage, mcp.updateModelContext): mcp_app_unknown, the app is not an MCP tool call with a UI in this conversation, or the resource is not an app's; mcp_server_mismatch, it names another server than the app's; mcp_tool_not_for_app, the tool is not listed with visibility including app; mcp_session_unavailable, the conversation has no open session of that server, or it ended; mcp_approval_denied and mcp_approval_expired, the person refused, or did not answer within x-mcpAppCallTiming.reviewDeadlineMs; mcp_cancelled, the review was withdrawn because the request was cancelled, the app was torn down, or the conversation ended, or, with no review withdrawn, the app's mount was released or the opening it was drawn in ended (closed, deleted, stopped, another begun, or the gateway stopping) before the request took effect, which for mcp.sendMessage sends nothing and opens nothing; mcp_request_too_large and mcp_result_too_large, past a request's bound (the arguments' 32 KiB, a message's input bound or its review's room, a context's 8 KiB) and a result's 56 KiB; turn_running also refuses an app's message while a turn runs or input waits; mcp_timed_out, the server did not answer in time; mcp_remote_error, the server answered with a JSON-RPC error (McpRemoteErrorDetails), or with something that is no MCP answer (no details). mcp_unauthorized, the remote endpoint refused the caller and the tool was not run; the answer carries no token. mcp_unreachable, the remote endpoint could not be reached and nothing was retained. mcp_insufficient_scope, the caller's scope was not enough and the call was not retried. These three are not mcp_session_unavailable: the session was not reported as ended. capabilities_refused: conversation.share asked Cedar's grant.comment or grant.drive before writing those capabilities and was denied. details.policies names the Cedar policy ids; it is empty when no permit matched. Nothing was written. The share does not match a gateway principal on its own. share_target_not_paired: conversation.share named a credential that is not an active paired device or peer gateway of the conversation's owner holding conversation.read. share_grant_changed: conversation.share or conversation.unshare named an expectedPolicyRevision the device's grant no longer holds: it was changed or revoked since that revision was read; nothing was changed, and conversation.shares says what it holds now. idempotency_conflict: conversation.message reused a requestId that already left another note (another text, delivery, surface or authority; the same grant at a later policy revision is the same authority); nothing was saved, and the first note stands. message_quota_exceeded: conversation.message found the conversation holding its most notes, or its author past its rate; nothing was saved. drive_policy_unenforceable: a Drive grant's conversation.send or conversation.steer where its tools cannot all be checked against the grant's tool policy (an OpenCode conversation, an approval mode other than ask, native hooks allowed, or the policy unreadable); nothing was saved or sent, and the same command fails the same way until the owner changes that. revoked: a Drive grant's conversation.send, or a steer that is queued rather than injected, whose grant was revoked or changed after the admitted revision was read and before the turn was saved; nothing was saved, the command receipt is settled as no_effect, and retrying that same request is answered revoked again. A new request is admitted only under the grant as it stands now. revoked_after_save: a native steer that was saved and then refused because the grant moved before injection; the saved attempt is settled as refused and nothing reached the agent. The command receipt is settled as refused_after_save, and retrying that same request is answered revoked_after_save again and is not admitted again. Retrying it as a different initiation is a submission conflict. unavailable_after_save: a native steer that was saved and then could not be checked because the grant record could not be read before injection; nothing reached the agent. The command receipt is settled as unavailable_after_save, and retrying that same request is answered unavailable_after_save again and is not admitted again. A new request is admitted only under the grant as it stands now. channel_not_found: conversation.create named a channelId the caller has no channel under; nothing was created. */
export const ConversationErrorCode = {
  AgentNotConfigured: "agent_not_configured",
  AgentUnsupported: "agent_unsupported",
  SandboxUnavailable: "sandbox_unavailable",
  EnvironmentNotConfigured: "environment_not_configured",
  EnvironmentUnavailable: "environment_unavailable",
  EnvironmentVersionMismatch: "environment_version_mismatch",
  EnvironmentPlatformUnsupported: "environment_platform_unsupported",
  EnvironmentInstallFailed: "environment_install_failed",
  LinkedFileUnreachable: "linked_file_unreachable",
  ModelUnavailable: "model_unavailable",
  ApprovalModeUnavailable: "approval_mode_unavailable",
  ApprovalModeNotApplied: "approval_mode_not_applied",
  ApprovalModeUncertain: "approval_mode_uncertain",
  ModelChangeNotApplied: "model_change_not_applied",
  ModelChangeUncertain: "model_change_uncertain",
  ApprovalRequestConflict: "approval_request_conflict",
  TurnRunning: "turn_running",
  ConversationsNotConfigured: "conversations_not_configured",
  UnknownMethod: "unknown_method",
  InvalidRequest: "invalid_request",
  ConversationNotFound: "conversation_not_found",
  ConversationCapacity: "conversation_capacity",
  ConversationClosed: "conversation_closed",
  ConversationConfigurationChanged: "conversation_configuration_changed",
  ConversationStateUnreadable: "conversation_state_unreadable",
  ConversationStorageUnavailable: "conversation_storage_unavailable",
  TemporarilyUnavailable: "temporarily_unavailable",
  AuditUnavailable: "audit_unavailable",
  SubmissionConflict: "submission_conflict",
  SubmissionUnresolved: "submission_unresolved",
  StalePermission: "stale_permission",
  AgentStartupDeadline: "agent_startup_deadline",
  AgentOperationFailed: "agent_operation_failed",
  ImageInputUnsupported: "image_input_unsupported",
  AttachmentNotFound: "attachment_not_found",
  AttachmentUnavailable: "attachment_unavailable",
  AttachmentCapacity: "attachment_capacity",
  AttachmentStorageUnavailable: "attachment_storage_unavailable",
  AttachmentCleanupUnavailable: "attachment_cleanup_unavailable",
  UploadTicketInvalid: "upload_ticket_invalid",
  UploadFailed: "upload_failed",
  UnsupportedImage: "unsupported_image",
  ImageTooLarge: "image_too_large",
  ConversationDeleted: "conversation_deleted",
  ConversationErasureIncomplete: "conversation_erasure_incomplete",
  McpAppUnknown: "mcp_app_unknown",
  McpServerMismatch: "mcp_server_mismatch",
  McpToolNotForApp: "mcp_tool_not_for_app",
  McpSessionUnavailable: "mcp_session_unavailable",
  McpApprovalDenied: "mcp_approval_denied",
  McpApprovalExpired: "mcp_approval_expired",
  McpCancelled: "mcp_cancelled",
  McpRequestTooLarge: "mcp_request_too_large",
  McpResultTooLarge: "mcp_result_too_large",
  McpTimedOut: "mcp_timed_out",
  McpRemoteError: "mcp_remote_error",
  McpUnauthorized: "mcp_unauthorized",
  McpUnreachable: "mcp_unreachable",
  McpInsufficientScope: "mcp_insufficient_scope",
  CapabilitiesRefused: "capabilities_refused",
  ShareTargetNotPaired: "share_target_not_paired",
  ShareGrantChanged: "share_grant_changed",
  IdempotencyConflict: "idempotency_conflict",
  MessageQuotaExceeded: "message_quota_exceeded",
  DrivePolicyUnenforceable: "drive_policy_unenforceable",
  Revoked: "revoked",
  RevokedAfterSave: "revoked_after_save",
  UnavailableAfterSave: "unavailable_after_save",
  ChannelNotFound: "channel_not_found",
} as const
export type ConversationErrorCode =
  (typeof ConversationErrorCode)[keyof typeof ConversationErrorCode]
/** One answer a question offers: the value recorded, and the label read. */
export interface ConversationAnswerOption {
  /** Exact value recorded when this option is chosen. */
  value: string
  /** Text shown to whoever answers. */
  label: string
  /** Secondary text, where the agent supplied any. */
  description?: string | null
}
/** One question, and what may be answered to it. */
export interface ConversationAsked {
  /** Key the answer is correlated back under. */
  key: string
  /** What is being asked. */
  prompt: string
  /** Short label for the question, where the agent supplied one. */
  header?: string | null
  /** Whether several options may be chosen rather than one. */
  multiSelect: boolean
  /** Whether an answer in the answerer's own words is accepted. */
  freeText: boolean
  /** Whether an answer must choose at least one of this question's options; own words alone do not satisfy it. Declining the whole ask is still possible. */
  required: boolean
  /** What may be chosen, in the order the agent offered it. */
  options: ConversationAnswerOption[]
}
/** A question the agent is waiting on an answer to, with all it asks, as conversation.questionReview reads it for one entry of a view. Nothing is authorised by answering; the agent simply cannot continue that path until it hears back. */
export interface ConversationQuestion {
  /** The key the view lists it under. An answer names it, so a question whose identities are reused is never answered by mistake. */
  question: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Identity the answer names. */
  questionId: string
  /** The agent's own framing of why it is asking. */
  message: string
  /** What is being asked, in the order the agent asked it. */
  questions: ConversationAsked[]
}
/** What was chosen for one question. Both parts may be absent, which is how a question is skipped. */
export interface ConversationQuestionChoice {
  /** The question this answers. */
  key: string
  /** Option values chosen, which the question must have offered. */
  values: string[]
  /** Words of the answerer's own, only where the question invited them. */
  ownWords?: string | null
}
/** Answer one question the agent asked, or decline to answer it. */
export interface ConversationAnswerQuestionParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** The question's key, as the view lists it. Refused with stale_permission when the question waiting under these identities has another. */
  question: string
  /** The ask being answered. */
  questionId: string
  /** What was chosen. Null declines the question, which is an answer the agent is told. */
  choices?: ConversationQuestionChoice[] | null
}
/** Nessa name of a binding-owned native approval preset. */
export const ApprovalMode = { Ask: "ask", Auto: "auto", Full: "full" } as const
export type ApprovalMode = (typeof ApprovalMode)[keyof typeof ApprovalMode]
/** One preset the binding can honor for this model. */
export interface ApprovalModeChoice {
  /** Nessa identifier for this binding-owned native preset. */
  id: ApprovalMode
  /** Short user-facing mode name. */
  name: string
  /** Precise provider-specific behavior without a universal approval promise. */
  description: string
}
/** One catalog model this configured agent can open. */
export interface AgentModelOption {
  /** Exact catalog model identifier. */
  modelId: string
  /** Catalog display name. */
  displayName: string
  /** Published context-window ceiling. */
  maxContextWindowTokens: number
  /** Catalog reasoning capability. */
  reasoning: boolean
  /** Catalog image-input capability. */
  imageInput: boolean
  /** Presets verified for this model. */
  approvalModes: ApprovalModeChoice[]
}
/** One agent configured on this gateway. */
export interface AgentOption {
  /** Agent identity. */
  agent: string
  /** Default catalog model identifier. */
  defaultModel: string
  /** Models runnable by this agent. */
  models: AgentModelOption[]
}
/** Turn this gateway's usage-limit continuation preference on or off. The preference is shared by every conversation on this gateway. Turning it on schedules a continuation for each currently blocked eligible turn. Turning it off cancels open schedules. */
export interface ConversationSetUsageLimitAutoContinueParams {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** True schedules continuations when a usage limit names a future reset. False cancels open schedules. The default, before any command, is false. */
  enabled: boolean
}
/** The preference now in effect for this gateway. */
export interface ConversationSetUsageLimitAutoContinueResult {
  /** Original action identifier. */
  requestId: string
  /** Whether continuations will be scheduled. */
  enabled: boolean
}
/** Allow or stop allowing one harness to run its own hooks. The next launch of that harness omits Nessa's suppression request when enabled is true. OpenCode is not a value of agent. */
export interface AgentsSetNativeHooksParams {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Claude or Codex. */
  agent: NativeHooksAgent
  /** True omits the suppression request the next time that harness's agent starts. False restores it. */
  enabled: boolean
}
/** The choice now in effect for the named harness. When enabled is true, hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts. Applies the next time each conversation's agent starts. */
export interface AgentsSetNativeHooksResult {
  /** Original action identifier. */
  requestId: string
  /** The harness this command changed. */
  agent: NativeHooksAgent
  /** The choice now in effect. */
  enabled: boolean
}
/** Configured agents and their model-specific choices. */
export interface AgentsListResult {
  /** Configured agents. */
  agents: AgentOption[]
  /** Places a new conversation may run besides this gateway: the SSH destinations config.json names, in the order it names them, then, while any paired environment is connected, `peer:any` (the gateway picks one of them for each opening of the conversation's agent and keeps it there while it serves and is not draining), then each paired environment connected when the catalog is read, as `peer:` and its key in lowercase hex. At most 16 of the first and 8 of the last. agents.list reads it once; agents.subscribe reads it again each time a paired environment connects, disconnects or is revoked. */
  environments: string[]
  /** Of environments, the ones a conversation owner's turns may run commands on with run: those the gateway's command policy allows the owner, and only where that environment takes commands, in the order environments lists them; never here. Read with environments, each time it is. These are the hosts a drive grant's ConversationToolPolicy.commandHosts can usefully name: a grant is never given more than the gateway allows, so a host not listed here runs a grant's commands nowhere. Empty when the gateway grants no commands. */
  commandHosts: string[]
  /** The stored choice the next process start of Claude and Codex reads, whether or not that agent is in this catalog. This is the catalog's only copy of it: agent rows do not repeat it. OpenCode has no entry and stays suppressed. agents.subscribe reads the catalog again each time this choice changes. */
  nativeHooks: AgentsNativeHooks
}
/** The choice in effect for the next process start of each harness. Off keeps Nessa's request to leave that harness's hooks off. On means hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts. For Claude, on also applies the repo's .claude settings, including its allow rules and startup hooks, so turn it on only for projects you trust. Applies the next time each conversation's agent starts. */
export interface AgentsNativeHooks {
  /** Claude's stored choice. */
  claude: NativeHooksChoice
  /** Codex's stored choice. */
  codex: NativeHooksChoice
}
/** The agent catalog as it is now, read after a paired environment connected, disconnected or was revoked, and sent only when it differs from the last one sent. Replace what was held with it. The subscription ends with conversation.subscriptionEnded, as a conversation subscription does. */
export interface AgentsListed {
  subscriptionId: ConversationSubscriptionId
  /** The same catalog agents.list returns, read again now. */
  catalog: AgentsListResult
}
/** Change a conversation approval preset using the native harness boundary; existing reviews are not answered by this command. */
export interface ConversationSetApprovalModeParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Preset requested for this conversation. */
  mode: ApprovalMode
}
/** Durably committed choice or typed refusal. */
export interface ConversationSetApprovalModeResult {
  /** Original action identifier. */
  requestId: string
  /** Preset durably committed by this command. */
  mode: ApprovalMode
}
/** A change in progress or awaiting recovery; the committed mode remains separate. */
export interface ApprovalModeChange {
  /** Correlated action identifier. */
  requestId: string
  /** Requested preset, kept separate from the committed mode during recovery. */
  requestedMode: ApprovalMode
  /** Turn admission state. */
  status: "changing" | "recovery_required"
}
/** Request installation of one pinned native runtime. The caller cannot select executable bytes. */
export interface AgentInstallParams {
  /** Agent whose pinned native runtime is being installed or offered. */
  agent: InstallableAgent
  /** Nonempty opaque invocation identity, bounded in UTF-8 bytes. Retained without truncation in the complete authenticated audit identity. */
  requestId: string
}
/** A native runtime this gateway can download for its host. */
export interface AgentInstallOffer {
  /** Agent whose pinned native runtime is being installed or offered. */
  agent: InstallableAgent
  /** Exact version verified by this Nessa build. */
  version: string
  /** Compressed download size in bytes. */
  archiveBytes: number
  /** The managed store currently verifies this release as installed. */
  installed: boolean
}
/** Current installation observations, independent of authentication. */
export interface AgentInstallOptionsResult {
  /** Supported native downloads; unsupported agents are absent. */
  agents: AgentInstallOffer[]
}
/** Acknowledged installation outcome. Authentication is checked separately. */
export interface AgentInstallResult {
  /** Agent whose pinned native runtime is being installed or offered. */
  agent: InstallableAgent
  /** Installed pinned version. */
  version: string
  /** This attempt fetched an archive. */
  downloaded: boolean
  /** Installation succeeded but superseded-runtime cleanup has warnings. */
  cleanupPending: boolean
}
/** Native runtimes the authenticated installer can offer. */
export const InstallableAgent = {
  Claude: "claude",
  Codex: "codex",
  Opencode: "opencode",
} as const
export type InstallableAgent = (typeof InstallableAgent)[keyof typeof InstallableAgent]
/** Exact receiver authority and physical stream identity. */
export interface RecordScope {
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  receiver: string
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  origin: string
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  stream: string
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  incarnation: string
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  schema: string
  /** Validated sync-engine Id, at most 128 UTF-8 bytes. */
  accessEpoch: string
}
/** Fixed target page after a downloaded checkpoint. */
export interface RecordPageRequest {
  /** Exact authorized receiver and physical stream identity. */
  scope: RecordScope
  /** Last downloaded position, as canonical unsigned decimal. */
  after: string
  /** Captured committed head for this pass, as canonical unsigned decimal. */
  target: string
  /** Maximum returned records, from one through the product bound. */
  maxRecords: number
  /** Maximum aggregate decoded physical payload bytes. */
  maxPayloadBytes: number
  /** Maximum decoded bytes in one physical record. */
  maxRecordBytes: number
}
/** One physical record; payload is canonical padded RFC 4648 base64. */
export interface RecordWireRecord {
  /** Dense physical position, starting at one. */
  position: string
  /** Immutable physical record identity. */
  id: string
  /** Canonical padded RFC 4648 base64 physical payload. */
  payload: string
}
/** Read a committed physical head under fresh receiver authority. */
export interface ConversationRecordsHeadParams {
  /** Conversation selected under authenticated ownership. */
  conversationId: string
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
  /** Trusted receiver binding selector; the actual physical scope is returned by the authenticated head read. */
  receiverId: string
}
/** Committed head for the exact requested scope. */
export interface ConversationRecordsHeadResult {
  /** Exact scope for this committed head. */
  scope: RecordScope
  /** Committed terminal physical position, as canonical unsigned decimal. */
  head: string
}
/** Read a bounded page through a captured terminal target. */
export interface ConversationRecordsPageParams {
  /** Conversation selected under authenticated ownership. */
  conversationId: string
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
  /** Bounded fixed-target page request. */
  request: RecordPageRequest
}
/** Echoed request and bounded physical records. */
export interface ConversationRecordsPageResult {
  /** Exact request echoed by the source. */
  request: RecordPageRequest
  /** One through sixteen contiguous physical records, validated before encoding. */
  records: RecordWireRecord[]
}
/** Typed refusal for bounded authorized record reads; transport loss is not a delivery acknowledgement. */
export const RecordReadErrorCode = {
  InvalidRequest: "invalid_request",
  Unauthorized: "unauthorized",
  Forbidden: "forbidden",
  WrongOwner: "wrong_owner",
  WrongReceiver: "wrong_receiver",
  StaleEpoch: "stale_epoch",
  Unverifiable: "unverifiable",
  IdentityChanged: "identity_changed",
  HistoryPruned: "history_pruned",
  RecordTooLarge: "record_too_large",
  ResponseTooLarge: "response_too_large",
  TemporarilyUnavailable: "temporarily_unavailable",
  ReadTimeout: "read_timeout",
  SourcePreparing: "source_preparing",
} as const
export type RecordReadErrorCode =
  (typeof RecordReadErrorCode)[keyof typeof RecordReadErrorCode]
/** Stable creation-order key; absence conveys no deletion. */
export interface CatalogueEntryKey {
  /** Creation revision of this stable identity. */
  creation: string
  /** Stable owner catalogue identity; text is preserved unchanged. */
  id: string
}
/** Current catalogue descriptor or explicit retained deletion marker. */
export interface CatalogueDescriptor {
  /** Stable creation-order identity of this descriptor. */
  key: CatalogueEntryKey
  /** Current committed revision, possibly newer than the captured pass boundary. */
  revision: string
  /** Explicit retained deletion marker for this identity. */
  deleted: boolean
}
/** Saved finite pass; the receiver owns durable progress and validates echoed correlation. */
export interface CataloguePass {
  /** Exact authorized owner scope saved with this finite pass. */
  scope: RecordScope
  /** Last fully completed owner revision. */
  completed: string
  /** Fixed head captured when this finite pass began. */
  boundary: string
  /** Last committed stable key; omitted before the first page. */
  cursor?: CatalogueEntryKey
  /** Positive receiver generation; delayed responses must match the saved pass. */
  generation: string
}
/** Bounded creation-order descriptor request. */
export interface CatalogueManifestRequest {
  /** Saved finite pass whose boundary and cursor select the next descriptors. */
  pass: CataloguePass
  /** Maximum manifest entries; sync-engine owns the accepted bound. */
  maxEntries: number
}
/** Discover catalogue scope and head after fresh authenticated owner admission. */
export interface ConversationCatalogueHeadParams {
  /** Server-bound receiver identity requesting this owner catalogue. */
  receiverId: string
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
}
/** Server-selected owner stream, physical incarnation, and head. */
export interface ConversationCatalogueHeadResult {
  /** Server-selected authorized owner scope, including current incarnation. */
  scope: RecordScope
  /** Current catalogue head as this receiver sees it: the latest revision among the conversations granted to it and its own grant changes. It moves on a grant, a revoke or a change to a granted conversation, never on the owner's other work. */
  head: string
}
/** Read a bounded manifest under exact freshly authorized scope. */
export interface ConversationCatalogueManifestParams {
  /** Saved finite pass and requested descriptor count. */
  request: CatalogueManifestRequest
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
}
/** Exact echoed request and current descriptors in stable creation order. */
export interface ConversationCatalogueManifestResult {
  /** Returned request evidence retained for receiver core correlation. */
  request: CatalogueManifestRequest
  /** Current descriptors returned in stable creation order. */
  entries: CatalogueDescriptor[]
  /** Whether eligible descriptors remain beyond this page. */
  hasMore: boolean
}
/** Resolve current value at the descriptor key and at least its revision. */
export interface ConversationCatalogueResolveParams {
  /** Saved finite pass authorizing this descriptor resolution. */
  pass: CataloguePass
  /** Manifest descriptor naming the key and minimum revision to resolve. */
  descriptor: CatalogueDescriptor
  /** Maximum decoded payload bytes; sync-engine owns the accepted bound. */
  maxPayloadBytes: number
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
}
/** Exact request correlation and current equal-or-newer value; no truncation. */
export interface ConversationCatalogueResolveResult {
  /** Returned pass evidence retained for receiver correlation. */
  pass: CataloguePass
  /** Returned requested descriptor evidence retained for receiver correlation. */
  descriptor: CatalogueDescriptor
  /** Current descriptor corresponding to the returned value or deletion marker. */
  entry: CatalogueDescriptor
  /** Base64 current metadata payload; empty for a retained deletion marker. */
  payload: string
}
/** Typed catalogue admission, source, capacity, and transport refusals. */
export const CatalogueReadErrorCode = {
  InvalidRequest: "invalid_request",
  Unauthorized: "unauthorized",
  Forbidden: "forbidden",
  WrongOwner: "wrong_owner",
  WrongReceiver: "wrong_receiver",
  StaleEpoch: "stale_epoch",
  Unverifiable: "unverifiable",
  IdentityChanged: "identity_changed",
  SourceUnavailable: "source_unavailable",
  OversizedEntry: "oversized_entry",
  ReadTimeout: "read_timeout",
  ResponseTooLarge: "response_too_large",
  ServerBusy: "server_busy",
} as const
export type CatalogueReadErrorCode =
  (typeof CatalogueReadErrorCode)[keyof typeof CatalogueReadErrorCode]
/** Discover the native phone endpoint and canonical consent under the current authenticated bootstrap session. */
export interface PhoneEndpointParams {}
/** Exact original authenticated bootstrap linkage. These public identifiers describe correlation; the server reconstructs current authority independently. */
export interface PhoneOwnerSource {
  /** Original conversation owner. */
  ownerId: string
  /** Original organization. */
  organizationId: string
  /** Original membership. */
  membershipId: string
  /** Original gateway audience. */
  audience: string
  /** Original public bootstrap credential identifier; not a token. */
  credentialId: string
  /** Composition-resolved original gateway resource. */
  resourceId: string
}
/** Complete ordered consent published by Auth. The envelope admits bounded grants; Auth owns the exact class, actions, order and resource correspondence. */
export interface PhoneConsent {
  /** Canonical access class returned by discovery. */
  class: string
  /** Complete canonical ordered action/resource set. No singular grant alias. */
  grants: ProductGrant[]
}
/** Authenticated discovery facts for signing one original phone authorization. No endpoint is inferred from a product address. */
export interface PhoneDiscovery {
  /** Original bootstrap identity and gateway resource. */
  source: PhoneOwnerSource
  /** Explicit usable numeric SocketAddr destination, as composed by the gateway. Auth validates the numeric address; the wire envelope bounds its text. */
  nativeEndpoint: string
  /** Exact gateway TLS SPKI bytes; Auth validates its key grammar. */
  gatewayKey: number[]
  /** Canonical complete consent returned by Auth. */
  consent: PhoneConsent
}
/** One immutable signed original. Auth validates nonzero request identity, endpoint, pin and canonical consent before effects. */
export interface PhoneOriginal {
  /** Original allocated nonzero request UUID bytes, retained through lost replies. */
  requestId: number[]
  /** Exact discovery included in the signed original. */
  discovery: PhoneDiscovery
  /** Exact permanent device public key. No private key is published. */
  deviceKey: number[]
}
/** Authorize this exact signed original under fresh bootstrap management and complete consent admission. Exact replay rejoins the same original. */
export interface PhoneAuthorizeParams {
  /** Original to verify and persist before activation. */
  original: PhoneOriginal
  /** Ed25519 signature of the Auth-owned original transcript. */
  signature: number[]
}
/** Find or rejoin only this exact original in the current bootstrap namespace. Digest mismatch or another source cannot disclose or activate it. */
export interface PhoneStatusParams {
  /** Original request UUID bytes. */
  requestId: number[]
  /** SHA-256 digest computed by the cryptographic owner from the original transcript. */
  originalDigest: number[]
}
/** Complete historical status of one original phone authorization. Auth constructs phase, receiver, credential and cleanup together; historical activation is not current read/write authority. */
export interface PhoneAuthorizationStatus {
  /** Exact stored signed original, including complete consent. */
  original: PhoneOriginal
  /** Auth-computed digest of this original transcript. */
  originalDigest: number[]
  /** Original invitation identity. */
  invitationId: number[]
  /** Original immutable consent identity. */
  consentId: number[]
  /** Original consent generation. */
  generation: number
  /** Original creation time, Unix milliseconds. */
  createdAtMs: number
  /** Original exclusive expiry, Unix milliseconds. */
  expiresAtMs: number
  /** Auth-owned current enrollment phase. */
  phase: PairingOwnerPhase
  /** Public issued or reserved device credential identifier, once known. */
  credentialId?: string
  /** Original receiver and access epoch, once known. */
  receiver?: PairingReceiver
  /** First terminal cause and initiator, once ended. */
  terminal?: PairingTerminal
  /** Whether original physical cleanup remains owed. */
  cleanupPending: boolean
  /** Typed activation interruption, when present. */
  activationStopped?: PairingActivationStop
}
/** Who an invitation enrolls. device: one of the owner's own devices; its credential acts as the owner and reads what is shared with it. ownerPhone: the owner's own phone; its credential acts as the owner over every conversation the owner has, now and later, reading and writing as the owner phone purpose allows, with no per-conversation share. gateway: a peer gateway that reads; its credential names a principal of kind gateway, identified by the key it pinned at pairing, and holds only the conversation.read grant. environment: a peer gateway or worker that serves as an environment for this gateway's leases; its credential names the same gateway principal for the same key and holds only the environment.serve grant, so it reads nothing. All use the same enrollment and listener; the choice is bound into the enrollment and cannot be changed by the enrolling side. */
export const PairingEnrollee = {
  Device: "device",
  OwnerPhone: "ownerPhone",
  Gateway: "gateway",
  Environment: "environment",
} as const
export type PairingEnrollee = (typeof PairingEnrollee)[keyof typeof PairingEnrollee]
/** Wire input for pairing.create. The owner, organization and gateway come from the session, never from the request. */
export interface PairingCreateParams {
  /** Who the invitation enrolls; absent means device. */
  enrollee?: PairingEnrollee
}
/** Wire input for pairing.status, pairing.deny and pairing.cancel: the invitation the owner names. The owner, organization and gateway come from the session, never from the request. */
export interface PairingInvitationParams {
  /** Random identity of the invitation, as its bytes. */
  invitationId: number[]
}
/** Wire input for pairing.approve. Approval records consent to this exact key, then stages, pairs a receiver and publishes the device's credential. */
export interface PairingApproveParams {
  /** Random identity of the invitation, as its bytes. */
  invitationId: number[]
  /** The exact device key the owner was shown as claimed, as its bytes. Any other key is refused. */
  deviceKey: number[]
}
/** Where an enrollment stands. available: a device can present the code. claimed: one device key completed the code exchange. approved: the owner consented to that key. staging: a credential and receiver are being prepared for it. active: the credential is issued; this is historical enrollment, not read authority. terminal: ended; see terminal. */
export const PairingOwnerPhase = {
  Available: "available",
  Claimed: "claimed",
  Approved: "approved",
  Staging: "staging",
  Active: "active",
  Terminal: "terminal",
} as const
export type PairingOwnerPhase = (typeof PairingOwnerPhase)[keyof typeof PairingOwnerPhase]
/** The first cause that ended the enrollment, kept across later cleanup. */
export const PairingTerminalCause = {
  CredentialRevoked: "credential_revoked",
  Denied: "denied",
  Cancelled: "cancelled",
  Expired: "expired",
  Restarted: "restarted",
} as const
export type PairingTerminalCause =
  (typeof PairingTerminalCause)[keyof typeof PairingTerminalCause]
/** Who ended the enrollment: the local operator, an authenticated principal, the enrolling device, or the gateway itself (expiry, restart). */
export const PairingInitiatorKind = {
  LocalOperator: "local_operator",
  Principal: "principal",
  Device: "device",
  System: "system",
} as const
export type PairingInitiatorKind =
  (typeof PairingInitiatorKind)[keyof typeof PairingInitiatorKind]
/** The actor that ended an enrollment. Exactly the field its kind names is present. */
export interface PairingInitiator {
  /** Which kind of actor. */
  kind: PairingInitiatorKind
  /** The principal, when kind is principal. */
  principalId?: string
  /** The device key, when kind is device. */
  deviceKey?: number[]
}
/** How an enrollment ended. */
export interface PairingTerminal {
  /** First cause that ended the enrollment. */
  cause: PairingTerminalCause
  /** Who caused it. */
  initiator: PairingInitiator
}
/** Receiver paired for an enrollment, with the access epoch of its original pair receipt; absent before a receiver is paired. */
export interface PairingReceiver {
  /** Receiver the device's reads are admitted through. */
  receiverId: string
  /** Receiver access epoch recorded with the enrollment. */
  accessEpoch: number
}
/** An owner's view of one enrollment. This is historical enrollment, not read authority: every later read is authorized again. */
export interface PairingOwnerStatus {
  /** Random identity of the invitation, as its bytes. */
  invitationId: number[]
  /** Identity of the immutable consent the invitation carries. */
  consentId: number[]
  /** Consent generation the device enrolls under. */
  generation: number
  /** Fixed class of access the consent is for, which also says who it enrolls: gateway-conversation-read for one of the owner's devices, peer-gateway-conversation-read for a peer gateway that reads, peer-gateway-environment-serve for a peer gateway or worker that serves as an environment. */
  class: string
  /** The exact action and resource the consent covers. */
  grant: ProductGrant
  /** When the invitation was created, Unix milliseconds. */
  createdAtMs: number
  /** Exclusive deadline for presenting the code, Unix milliseconds. */
  expiresAtMs: number
  /** Where the enrollment stands. */
  phase: PairingOwnerPhase
  /** The device key that claimed the invitation, once one has. Approval must name exactly this key. */
  claimedDeviceKey?: number[]
  /** Credential reserved for the device, once staging has begun. */
  credentialId?: string
  /** Receiver recorded for the enrollment, once known. */
  receiver?: PairingReceiver
  /** How the enrollment ended, when phase is terminal. */
  terminal?: PairingTerminal
  /** Whether physical cleanup of a staged receiver is still owed. Read from the record, never stored separately. */
  cleanupPending: boolean
}
/** Why an approval stopped before active. retryable: a failure that can clear (storage, receiver or worker unavailable, another approval of the same enrollment in progress, or the owner's session expired or membership is inactive, cleared by signing in again or an admin re-enabling the membership); approving again continues from status. permanent: approving again would stop the same way (the receiver no longer holds the pairing, the owner no longer holds the grant, a conflicting record); cancel the enrollment and pair again. */
export const PairingActivationStop = {
  Retryable: "retryable",
  Permanent: "permanent",
} as const
export type PairingActivationStop =
  (typeof PairingActivationStop)[keyof typeof PairingActivationStop]
/** Result of pairing.approve: the enrollment after approval and activation, and why activation stopped if it did. */
export interface PairingApproveResult {
  /** The enrollment as it now stands. The approval itself is committed whatever else happened. */
  status: PairingOwnerStatus
  /** Present when activation stopped before active: whether approving again can finish it. */
  activationStopped?: PairingActivationStop
}
/** Result of pairing.create: a code for one device, valid until status.expiresAtMs. */
export interface PairingCreateResult {
  /** The one-time code in its grouped display form, two groups joined by a hyphen. Shown once: it is not stored and cannot be read again. A lost code means cancelling the invitation and creating another. */
  code: string
  /** The invitation as committed. */
  status: PairingOwnerStatus
}
/** Result of pairing.pending: the enrollments an owner can still act on, without any code. */
export interface PairingPendingResult {
  /** The caller's unfinished enrollments, including ended ones still owed cleanup. Bounded by the registry's configured capacity rather than a wire constant. */
  items: PairingOwnerStatus[]
}
/** Turn native linking on or off for this gateway. The gateway saves the choice in its config.json and, when the running gateway does not already match it, restarts itself to apply it. Turning it on keeps a listen address already named there, or names 127.0.0.1:47650; turning it off keeps the native settings and marks them off. */
export interface PairingSetLinkingParams {
  /** True turns native linking on; false turns it off. Sending the same value again changes nothing, so a retry is safe. */
  enabled: boolean
}
/** How the choice takes effect. unchanged: the running gateway already matches it. restarting: the gateway is restarting itself now; the connection closes and reconnects to a gateway that matches it. A signal that stops the gateway before it restarts wins: it stays stopped, and the choice applies the next time it starts. */
export const PairingLinkingApply = {
  Unchanged: "unchanged",
  Restarting: "restarting",
} as const
export type PairingLinkingApply =
  (typeof PairingLinkingApply)[keyof typeof PairingLinkingApply]
/** Result of pairing.setLinking. */
export interface PairingSetLinkingResult {
  /** The saved choice. */
  enabled: boolean
  /** How the saved choice takes effect. */
  applies: PairingLinkingApply
}
/** How the web tunnel is carried. quick: a free Cloudflare quick tunnel at a random trycloudflare.com address, which changes whenever cloudflared starts again (it is kept running across gateway restarts, so it changes only when cloudflared itself has to start again). named: a tunnel made in the person's Cloudflare account, at their own hostname, which never changes. */
export const PairingTunnelMode = { Quick: "quick", Named: "named" } as const
export type PairingTunnelMode = (typeof PairingTunnelMode)[keyof typeof PairingTunnelMode]
/** Turn the web tunnel on or off for this gateway, so a paired phone reaches it from anywhere. Needs native linking on. The gateway saves the choice in its config.json (the token in a private file beside its state, never in config.json) and, when the running gateway does not already match it, restarts itself to apply it. A field left out keeps what is saved. */
export interface PairingSetTunnelParams {
  /** True turns the tunnel on; false turns it off and keeps its other settings. Sending the same request again changes nothing, so a retry is safe. */
  enabled: boolean
  /** Quick or named. Left out, the saved mode, or quick when none is saved. */
  mode?: PairingTunnelMode
  /** A named tunnel's public address, https://host or https://host:port, with no path. Left out, the saved one. */
  hostname?: string
  /** A named tunnel's token from Cloudflare's dashboard: printable ASCII with no spaces. Left out, the saved one. It is never sent back. */
  token?: string
}
/** Result of pairing.setTunnel. */
export interface PairingSetTunnelResult {
  /** The saved choice. */
  enabled: boolean
  /** The saved mode. */
  mode: PairingTunnelMode
  /** How the saved choice takes effect. */
  applies: PairingLinkingApply
}
/** Where the running gateway's tunnel stands. off: this run has no tunnel. starting: it is being started, or started again. ready: it answers at address. failed: the last attempt failed for failure; it is tried again. */
export const PairingTunnelStatus = {
  Off: "off",
  Starting: "starting",
  Ready: "ready",
  Failed: "failed",
} as const
export type PairingTunnelStatus =
  (typeof PairingTunnelStatus)[keyof typeof PairingTunnelStatus]
/** Result of pairing.tunnel: the saved tunnel setting and the running tunnel. The token is never included. */
export interface PairingTunnelResult {
  /** Whether the saved setting turns the tunnel on. */
  enabled: boolean
  /** The saved mode, quick when none is saved. */
  mode: PairingTunnelMode
  /** The saved named tunnel's address, when one is saved. */
  hostname?: string
  /** Whether a named tunnel's token is saved. */
  hasToken: boolean
  /** The loopback address the tunnel forwards to: where a named tunnel's public hostname must point in Cloudflare's dashboard, as http://<ingress>. */
  ingress: string
  /** The running tunnel's status. */
  status: PairingTunnelStatus
  /** The running tunnel's public address, when it is ready. */
  address?: string
  /** Why the last attempt failed, when status is failed: unsupported (no pinned cloudflared for this machine), download, unverified (the download did not match its pin), storage, token, ingress (the loopback ingress could not be bound), start or not_ready. */
  failure?: string
}
/** Result of server.reach: the addresses this gateway can be reached at beyond the local network, for a paired device to keep. */
export interface ServerReachResult {
  /** The web tunnel's public address, https://host[:port], when a tunnel is ready; null otherwise. Open wss://<host>/native and speak the native connection inside its binary messages. */
  tunnel: string | null
}
/** Why the gateway refused a pairing method it dispatched. pairing_not_configured: native pairing is off in this gateway's configuration. pairing_not_found: no such invitation. pairing_slot_occupied: an invitation is already open; cancel it first. pairing_capacity: too many unfinished enrollments. pairing_conflict: the request names a different key or outcome than the one recorded. pairing_ineligible: the enrollment cannot take this step now (expired, ended, or not yet claimed). pairing_busy: another create is running; or, for pairing.setLinking and pairing.setTunnel, config.json is locked by another change or the gateway is already restarting or stopping, and nothing was saved. pairing_unavailable: storage or a worker failed; retry later. pairing_address_in_use: pairing.setLinking could not turn linking on because its listen address cannot be bound on this machine (another program holds the port, or the address is not one of this machine's); nothing was saved. pairing_audit_unavailable: pairing.setLinking could not keep the change's audit record; if that was before the change nothing was saved, and if after it the choice is saved and the gateway did not restart, so it applies the next time the gateway starts (read the state again to see which). pairing.setTunnel refuses with the same codes: pairing_busy and pairing_unavailable as above, pairing_address_in_use when the tunnel's loopback ingress cannot be bound, and pairing_audit_unavailable as above. pairing_linking_off: pairing.setTunnel cannot turn the tunnel on while native linking is off; nothing was saved. pairing_tunnel_incomplete: a named tunnel needs a hostname and a token, given or saved; nothing was saved. */
export const PairingErrorCode = {
  PairingNotConfigured: "pairing_not_configured",
  PairingNotFound: "pairing_not_found",
  PairingSlotOccupied: "pairing_slot_occupied",
  PairingCapacity: "pairing_capacity",
  PairingConflict: "pairing_conflict",
  PairingIneligible: "pairing_ineligible",
  PairingBusy: "pairing_busy",
  PairingUnavailable: "pairing_unavailable",
  PairingAddressInUse: "pairing_address_in_use",
  PairingAuditUnavailable: "pairing_audit_unavailable",
  PairingLinkingOff: "pairing_linking_off",
  PairingTunnelIncomplete: "pairing_tunnel_incomplete",
} as const
export type PairingErrorCode = (typeof PairingErrorCode)[keyof typeof PairingErrorCode]
/** The GitHub account a sign-in belongs to. */
export interface GithubAccount {
  /** GitHub's numeric id for the account, which never changes. */
  id: number
  /** The account's login, which can change. */
  login: string
}
/** Where the gateway's GitHub sign-in stands. signed_out: no token is held, and agents are given none. connecting: a code is waiting for the person to enter it on GitHub. connected: a token is held and every agent opened from now on is given it. */
export const GithubConnectState = {
  SignedOut: "signed_out",
  Connecting: "connecting",
  Connected: "connected",
} as const
export type GithubConnectState =
  (typeof GithubConnectState)[keyof typeof GithubConnectState]
/** Why the latest sign-in ended without signing in. expired: the code ran out before it was entered. denied: the person refused it on GitHub. cancelled: it was cancelled here. rejected: GitHub refused the Nessa app itself. unreachable: GitHub could not be reached. store_unavailable: the token could not be kept, or a kept one could not be read at startup. audit_unavailable: its record could not be kept, so the token was dropped. */
export const GithubConnectEnd = {
  Expired: "expired",
  Denied: "denied",
  Cancelled: "cancelled",
  Rejected: "rejected",
  Unreachable: "unreachable",
  StoreUnavailable: "store_unavailable",
  AuditUnavailable: "audit_unavailable",
} as const
export type GithubConnectEnd = (typeof GithubConnectEnd)[keyof typeof GithubConnectEnd]
/** The code a sign-in waits for. */
export interface GithubCode {
  /** What the person enters on GitHub. */
  userCode: string
  /** Where they enter it. */
  verificationUri: string
  /** Whole seconds left before the code expires, when this was answered. */
  expiresInSeconds: number
}
/** Live delivery is opted in for this gateway's signed-in GitHub account. Gaps report expired catch-up history; they are not acknowledged automatically. */
export interface GithubLiveEvents {
  enabled: boolean
  repositories: number
  gaps: number
}
export interface GithubLiveEventsParams {
  enabled: boolean
  /** Explicit acknowledgement of missing history, continue within retained events. */
  acceptGaps?: boolean
}
/** Result of github.status, github.connect, github.cancel and github.disconnect: where the gateway's GitHub sign-in stands now. The gateway signs in through the Nessa GitHub App with the device flow, keeps the token in its passphrase-protected local vault, and gives it to each agent it opens from then on as GH_TOKEN and a git credential helper for https://github.com. */
export interface GithubConnection {
  /** Where the sign-in stands. */
  state: GithubConnectState
  /** Present when this build stage has a live relay configured. */
  liveEvents?: GithubLiveEvents
  /** Present exactly when connected. */
  account?: GithubAccount
  /** Present exactly when connecting. */
  code?: GithubCode
  /** When signed out: why the latest sign-in ended, until another starts. Absent when there was none, or after a disconnect. */
  last?: GithubConnectEnd
  /** The gateway could not keep the last change in its audit, and that change still stands. With last: how the attempt ended. Connected: a sign-out that failed. Signed out with no last: the sign-out. Absent when it is on record, and with last audit_unavailable, which already says so. */
  unrecorded?: true
  /** Where the person chooses the repositories the Nessa app may use. A token reaches only those. */
  installUrl: string
}
/** Why the gateway refused a github method. github_not_configured: this gateway signs in to nothing (no agents run here). github_unreachable: github.connect could not reach GitHub for a code; nothing started. github_rejected: GitHub refused the Nessa app; nothing started. github_storage_unavailable: the encrypted local vault could not complete the requested sign-in operation; a sign-in that was held is still held. github_audit_unavailable: a record could not be kept; before a change nothing changed, and after github.disconnect deleted the token or github.cancel stopped the code, that stands (read github.status). */
export const GithubErrorCode = {
  GithubNotConfigured: "github_not_configured",
  GithubUnreachable: "github_unreachable",
  GithubRejected: "github_rejected",
  GithubStorageUnavailable: "github_storage_unavailable",
  GithubAuditUnavailable: "github_audit_unavailable",
  GithubLiveUnavailable: "github_live_unavailable",
  GithubLiveCapacity: "github_live_capacity",
  VaultLocked: "vault_locked",
} as const
export type GithubErrorCode = (typeof GithubErrorCode)[keyof typeof GithubErrorCode]
/** Wire input for peer.enroll. This gateway enrolls with its own native key, so the peer knows it as one principal however often it is enrolled. */
export interface PeerEnrollParams {
  /** The peer gateway's native listen address, as an IP address and port (`192.0.2.7:7443`, `[2001:db8::7]:7443`). No name is resolved. */
  address: string
  /** The one-time code the peer's owner was shown for a gateway invitation, in its grouped display form. */
  code: string
  /** Where to ask for the peer when its address does not answer: a relay's `host:port`, the host a name or an address (an IPv6 one in brackets), at most 255 UTF-8 bytes (`x-utf8MaxBytes`; `maxLength` counts code points and is only a coarse bound). Looked up at each dial. It carries no trust: the peer must still present its pinned key through it. Enrollment itself always dials the address; the relay is kept for reading the peer afterwards. Absent: none. */
  relay?: string
}
/** Wire input for peer.forget: the peer to forget, named by its key. */
export interface PeerForgetParams {
  /** The peer's key, as peer.list shows it. */
  peerKey: number[]
}
/** Wire input for peer.relay: set or clear the relay kept for a peer. A read of that peer in progress is stopped first, and the next read uses the new relay. */
export interface PeerRelayParams {
  /** The peer's key, as peer.list shows it. */
  peerKey: number[]
  /** Where to ask for the peer when its address does not answer: a relay's `host:port`, the host a name or an address (an IPv6 one in brackets), at most 255 UTF-8 bytes (`x-utf8MaxBytes`; `maxLength` counts code points and is only a coarse bound). Looked up at each dial. It carries no trust: the peer must still present its pinned key through it. Null clears it, so the peer is read only at its address. */
  relay: string | null
}
/** Where this gateway's enrollment with a peer stands, as this gateway last heard. pending: claimed, waiting on the peer's owner to approve, or not yet confirmed. active: the peer issued this gateway a credential. revoked: the peer ended the enrollment (its owner revoked the credential, or denied, cancelled or let expire the claim), its retained cache is removed and it is no longer read; only peer.forget helps. unreadable: the peer's record is there but this build cannot read it, or it was made with a key that is no longer this gateway's; only peer.forget helps. */
export const PeerPhase = {
  Pending: "pending",
  Active: "active",
  Revoked: "revoked",
  Unreadable: "unreadable",
} as const
export type PeerPhase = (typeof PeerPhase)[keyof typeof PeerPhase]
/** One peer gateway this gateway enrolled into. Result of peer.enroll, peer.relay and peer.forget; peer.list adds sync for a pending or active peer. */
export interface PeerGateway {
  /** The peer's key: what identifies the peer, whatever address it answers at. Its TLS key is pinned to exactly this. */
  peerKey: number[]
  phase: PeerPhase
  /** The address that last answered as this peer. Absent when the record is unreadable. */
  address?: string
  /** The relay this gateway asks for the peer at when its address does not answer. Absent when none is set, or the record is unreadable. */
  relay?: string
  /** The credential the peer issued this gateway, once active. An identifier, not a secret. */
  credentialId?: string
  /** The receiver the peer paired with that credential, once active. */
  receiverId?: string
  /** What this gateway's poller last saw reading the peer. Only in peer.list, and only for a pending or active peer; absent for a revoked or unreadable one, and in the results of peer.enroll and peer.forget. */
  sync?: PeerSync
}
/** How reading a peer last went, since this gateway started. waiting: not read yet, or still pending on the peer. synced: the last read brought the retained cache up to what the peer grants. syncing: the last read saved some of what the peer grants and stopped at a bound (a page bound or its time budget); the next continues it. A read an owner command stops leaves the state as it was. unreachable: the peer could not be reached, or the last read ran out of its time budget before it saved anything; reads back off. away: the peer's address did not answer and its relay said the peer is not registered with it (asleep, quit or offline, as far as the relay can tell; nothing authenticates that word); the retained cache is kept and readable, and reads back off as for unreachable. failed: the peer answered but the read failed; reads back off. quota: this gateway's retained cache of the peer is full; each read still withdraws what the peer no longer grants, and reads back off until the peer grants less. */
export const PeerSyncState = {
  Waiting: "waiting",
  Synced: "synced",
  Syncing: "syncing",
  Unreachable: "unreachable",
  Away: "away",
  Failed: "failed",
  Quota: "quota",
} as const
export type PeerSyncState = (typeof PeerSyncState)[keyof typeof PeerSyncState]
/** What this gateway last saw reading a peer: its poller reads each pending or active peer's status and grants on a cadence. */
export interface PeerSync {
  state: PeerSyncState
  /** When the last read of this peer that saved something, or found the cache up to date, finished, in Unix milliseconds. A failed read leaves it as it was. Absent until one has since this gateway started. */
  lastSyncedAtMs?: number
  /** Conversations the peer granted that this gateway's retained cache holds, as the last read counted. Absent until one has. */
  conversations?: number
}
/** Result of peer.list. */
export interface PeerListResult {
  /** Every peer this gateway keeps, by key. */
  items: PeerGateway[]
}
/** Why the gateway refused a peer method it dispatched. peer_not_configured: native pairing is off in this gateway's configuration, so it has no key to enroll with. peer_not_found: no peer with that key. peer_exists: a record for that peer is already kept; forget it first. peer_capacity: this gateway keeps as many peers as it may. peer_busy: another enrollment or forget is running; retry once it ends. A read of a peer makes a command busy only if a stopped read does not give back its turn within peer.preempt: the command stops the read and takes its turn once the read gives it back, and the read carries on later. peer_unreachable: nothing answered at the address, the connection failed, or what answered is not a gateway enrolling peers; an open port is not told from a closed one. peer_invitation_refused: the peer refused the code: no open invitation, or an expired, used or wrong code; it does not say which. peer_wrong_invitation: the code is for a device, not a gateway. peer_own_gateway: the address is this gateway's own. peer_unavailable: storage, the key or a worker failed, or the gateway is stopping and refused the command before any effect; retry later. peer_audit_unavailable: the command's audit record could not be kept; if that was before its effect nothing changed, and if after, the effect stands and peer.list shows it. */
export const PeerErrorCode = {
  PeerNotConfigured: "peer_not_configured",
  PeerNotFound: "peer_not_found",
  PeerExists: "peer_exists",
  PeerCapacity: "peer_capacity",
  PeerBusy: "peer_busy",
  PeerUnreachable: "peer_unreachable",
  PeerInvitationRefused: "peer_invitation_refused",
  PeerWrongInvitation: "peer_wrong_invitation",
  PeerOwnGateway: "peer_own_gateway",
  PeerUnavailable: "peer_unavailable",
  PeerAuditUnavailable: "peer_audit_unavailable",
} as const
export type PeerErrorCode = (typeof PeerErrorCode)[keyof typeof PeerErrorCode]
/** Opaque identity of one subscription on this connection. It grants nothing and is discarded with the connection. */
export type ConversationSubscriptionId = string
/** Where in the committed history a view was read. Compare positions only within one incarnation. */
export interface ConversationViewCursor {
  /** The stored history's incarnation. A different one means the history was replaced, and the frame is a replacement for everything held. */
  incarnation: string
  /** Decimal committed position the view was folded through, within this incarnation. */
  position: string
}
/** Follow one conversation's view: the reply comes after the first read, then a conversation.view frame whenever what the view shows changes, until conversation.subscriptionEnded or conversation.unsubscribe. */
export interface ConversationSubscribeParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The cursor of the last view this client applied. No frame behind it in the same incarnation is sent; at least one frame at or past it is. */
  after?: ConversationViewCursor
}
/** Follow the caller's conversation list: the reply comes after the first list, then a conversation.listed frame whenever the list changes. */
export interface ConversationSubscribeListParams {
  /** Follow only archived conversations when true; only unarchived ones when false or absent. */
  archived?: boolean
}
/** The subscription is live. Its first frame follows this reply. */
export interface ConversationSubscribeResult {
  subscriptionId: ConversationSubscriptionId
}
/** Stop one subscription of this connection: conversation.unsubscribe stops one made by conversation.subscribe or conversation.subscribeList, agents.unsubscribe one made by agents.subscribe, and either answers unknown_subscription for the other's. A frame already being written may arrive before the reply; none arrives after it. */
export interface ConversationUnsubscribeParams {
  subscriptionId: ConversationSubscriptionId
}
/** The conversation's current view, read after a committed or live change. Replace what was held with it. */
export interface ConversationViewed {
  subscriptionId: ConversationSubscriptionId
  cursor: ConversationViewCursor
  /** The same bounded replacement conversation.read returns. */
  view: ConversationView
}
/** The caller's current conversation list, read after a change. Replace what was held with it. */
export interface ConversationListed {
  subscriptionId: ConversationSubscriptionId
  /** The same list conversation.list returns, cut newest first and marked incomplete when it would not fit one frame. */
  list: ConversationListResult
}
/** Why a subscription ended. lagging: a frame waited longer than x-subscriptionLimits.deliveryTimeoutMs to be written; subscribe again from lastDelivered. refused: a read or its access check was refused, with code. source_closed: the change source closed; subscribe again. too_large: a frame would not fit the frame bound. */
export const ConversationSubscriptionEndReason = {
  Lagging: "lagging",
  Refused: "refused",
  SourceClosed: "source_closed",
  TooLarge: "too_large",
} as const
export type ConversationSubscriptionEndReason =
  (typeof ConversationSubscriptionEndReason)[keyof typeof ConversationSubscriptionEndReason]
/** The subscription's one terminal frame. Nothing of it follows. */
export interface ConversationSubscriptionEnded {
  subscriptionId: ConversationSubscriptionId
  reason: ConversationSubscriptionEndReason
  /** For refused: the conversation error code, or unauthorized or forbidden when access was refused. */
  code?: string
  /** Last semantic event cursor written, or the supplied after cursor if no batch was written. A written cursor records socket delivery, not application processing. Resume from your application's own last-applied batch cursor. */
  lastEventCursor?: ConversationEventCursor
  /** Where a view subscription resumes from: the cursor of the last view frame written or, when none was, the after it was opened with. Absent for a list, and for a view subscription opened without after that wrote no frame. */
  lastDelivered?: ConversationViewCursor
}
/** Refusals of subscribe and unsubscribe beside the conversation error codes a refused first read carries. subscription_capacity: the connection's published limit is full. subscription_duplicate: this connection already follows that target; the agent catalog is one target, so a connection follows it at most once. unknown_subscription: no such live subscription on this connection, or one the other unsubscribe stops. cursor_ahead: after names a position past the stored history. */
export const ConversationSubscriptionErrorCode = {
  SubscriptionCapacity: "subscription_capacity",
  SubscriptionDuplicate: "subscription_duplicate",
  UnknownSubscription: "unknown_subscription",
  CursorAhead: "cursor_ahead",
} as const
export type ConversationSubscriptionErrorCode =
  (typeof ConversationSubscriptionErrorCode)[keyof typeof ConversationSubscriptionErrorCode]
/** What becomes of a note once it is saved. record_only saves it with its author and never hands it to the model by itself. next_turn saves it pending and hands it to the model, labelled with its author, with the next turn a person starts with conversation.send while the agent is idle: never with a steer, a turn queued behind another, or an app's message. That turn takes at most 8 notes, the oldest first, and only those whose author still holds the right to leave them, checked as the turn is admitted; a note whose author's grant was revoked or lowered below comment is retired permission_lost instead, and one still pending 24 hours after it was accepted is retired expired. */
export const ConversationMessageDelivery = {
  RecordOnly: "record_only",
  NextTurn: "next_turn",
} as const
export type ConversationMessageDelivery =
  (typeof ConversationMessageDelivery)[keyof typeof ConversationMessageDelivery]
/** Where a saved note stands. recorded is a record_only note: saved, with nothing waiting to deliver it, and it stays recorded. pending is a next_turn note waiting for a turn to take it. assigned is a next_turn note a turn took, named by executionId; it is never pending again, whatever becomes of that turn. retired is a next_turn note no turn will take, for retiredReason. */
export const ConversationMessageState = {
  Recorded: "recorded",
  Pending: "pending",
  Assigned: "assigned",
  Retired: "retired",
} as const
export type ConversationMessageState =
  (typeof ConversationMessageState)[keyof typeof ConversationMessageState]
/** Why a next_turn note was retired. expired: it was still pending 24 hours after it was accepted. permission_lost: when a turn was admitted, its author's grant had been revoked or lowered below comment. */
export const ConversationMessageRetiredReason = {
  Expired: "expired",
  PermissionLost: "permission_lost",
} as const
export type ConversationMessageRetiredReason =
  (typeof ConversationMessageRetiredReason)[keyof typeof ConversationMessageRetiredReason]
/** One saved record this read could not apply. The transcript draws one line for it, after the turn named by afterExecutionId, and that line is where a future report action attaches. */
export interface ConversationUnreadable {
  /** The record's physical position, as a decimal with no leading zero. */
  id: string
  /** The last turn accepted when the record was skipped, so the line is shown after that turn; absent when none had been. */
  afterExecutionId?: string
}
/** One note left in the conversation, as everyone who reads it sees it. A note is not a turn and not the person's message: show it apart from the turns, with its author. */
export interface ConversationNote {
  /** The conversation's own name for the note, as its receipt gave it. */
  messageId: string
  /** What the note says, exactly as it was left. */
  text: string
  /** When the note was accepted, in milliseconds since the Unix epoch, on the gateway's clock. */
  acceptedAtMs: number
  /** What it was left for. */
  delivery: ConversationMessageDelivery
  /** Where it stands. */
  deliveryState: ConversationMessageState
  /** The turn that took it; present exactly when deliveryState is assigned. */
  executionId?: string
  /** Why it was retired; present exactly when deliveryState is retired. */
  retiredReason?: ConversationMessageRetiredReason
  /** The last turn the conversation had accepted when the note was, so it is shown after that turn; absent when it was left before any turn. */
  afterExecutionId?: string
  /** Who left it. */
  author: ConversationNoteAuthor
}
/** owner: the conversation's owner. grant: a share grant holder. agent: another turn's agent. */
export const ConversationNoteAuthorKind = {
  Owner: "owner",
  Grant: "grant",
  Agent: "agent",
} as const
export type ConversationNoteAuthorKind =
  (typeof ConversationNoteAuthorKind)[keyof typeof ConversationNoteAuthorKind]
/** Under which authority a note was left, and from which surface: the label it carries in the transcript and to the model. */
export interface ConversationNoteAuthor {
  /** Under which authority. */
  kind: ConversationNoteAuthorKind
  /** The verified surface it was left from. */
  surface: string
  /** The verified source turn when kind is agent. */
  agent?: ConversationAgentSource
}
/** A point after which the conversation's agent ran in a new session that had not seen the turns before it: a switch to another agent or model (conversation.switchAgent), or the fork the conversation began as (conversation.fork). The first turn after it was given those turns as text. Show it as a line in the transcript, after the turn it follows. */
export interface ConversationHandoff {
  /** The conversation's own name for the handoff. */
  handoffId: string
  /** A switch to another agent or model, or the fork the conversation began as. */
  kind: "switch" | "fork"
  /** When it was recorded, in milliseconds since the Unix epoch, on the gateway's clock. */
  atMs: number
  /** The last turn the conversation held when it was recorded, which it is shown after; absent before any turn, as a fork always is. */
  afterExecutionId?: string
  /** A switch's agent and model before it; present exactly for a switch. */
  from?: ConversationHandoffAgent
  /** A switch's agent and model after it; present exactly for a switch. */
  to?: ConversationHandoffAgent
  /** A fork's copied turns, oldest first, shown before everything the conversation said itself. Absent for a switch, and when the view left them all out. */
  turns?: ConversationCarriedTurn[]
  /** Copied turns left out, by the copy's own bounds or by this view's; absent when none were. */
  omittedTurns?: number
}
/** An agent and model a switch left or went to. */
export interface ConversationHandoffAgent {
  /** The agent, as agents.list names it; absent when the history's provider is one this gateway has no agent for. */
  agent?: string
  /** The model, as the agent's catalog names it. */
  model: string
}
/** One turn a fork copied, as the new conversation's agent was given it: what was said, and the agent's final reply. Text only. */
export interface ConversationCarriedTurn {
  /** What was said in the turn, when it said anything in text. */
  said?: string
  /** The agent's final reply, when it gave one. */
  reply?: string
}
/** Leave a note in one conversation (conversation.message). A note is saved with its author, the caller's principal and surface and the authority it was admitted under, and starts or steers no turn: a next_turn note waits for the next turn a person starts (see ConversationMessageDelivery). The conversation's owner leaves one from its own surfaces; a paired device needs a comment or drive grant on that conversation, and is refused forbidden under a read grant and conversation_not_found where it holds none. The same requestId with the same text and delivery, from the same surface under the same authority, answers the first receipt unchanged, however long after; any other note under that requestId is refused idempotency_conflict. A conversation keeps at most 1024 notes, and one author (a principal under one authority) leaves at most 20 in any 60 seconds; past either it is refused message_quota_exceeded and nothing is saved. At most 64 next_turn notes wait for a turn at once, not counting one past its 24 hours, which the next turn retires; past that a next_turn note is refused message_quota_exceeded too. A deleted conversation refuses it conversation_deleted. */
export interface ConversationMessageParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** What the note says: 1 to 8192 bytes, not all blank, kept exactly as given. */
  text: string
  /** What becomes of the note once it is saved. */
  delivery: ConversationMessageDelivery
}
/** The receipt of a saved note. A retry of the same request answers the same note, where it stands then. */
export interface ConversationMessageReceipt {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The conversation's own name for the note, made when it was accepted. */
  messageId: string
  /** When the note was accepted, in milliseconds since the Unix epoch, on the gateway's clock. */
  acceptedAtMs: number
  /** Where the note stands now. */
  deliveryState: ConversationMessageState
  /** The turn that took the note; present exactly when deliveryState is assigned. */
  executionId?: string
  /** Why the note was retired; present exactly when deliveryState is retired. */
  retiredReason?: ConversationMessageRetiredReason
}
/** Read where the caller's own note stands (conversation.messageStatus), by the requestId it was left under. This writes nothing. It is admitted as conversation.message is, so a paired device needs a comment or drive grant; it finds notes by principal and requestId, so a device signed in as its owner finds the owner's notes and those of the owner's other devices, and never another principal's. */
export interface ConversationMessageStatusParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
}
/** Whether the caller left a saved note under that request, and where it stands. found is false when it left no note that is saved yet. After a conversation.message that failed, a note can still be saved by a later write, so false does not make a new note safe: send the same requestId again, which saves it at most once. */
export interface ConversationMessageStatusResult {
  /** True when a saved note answers this request. */
  found: boolean
  /** The request that was looked up. */
  requestId: string
  /** Present when found is true. */
  messageId?: string
  /** Present when found is true. */
  acceptedAtMs?: number
  /** Present when found is true. */
  deliveryState?: ConversationMessageState
  /** The turn that took the note; present exactly when deliveryState is assigned. */
  executionId?: string
  /** Why the note was retired; present exactly when deliveryState is retired. */
  retiredReason?: ConversationMessageRetiredReason
}
/** Whether a remembered rule allows or denies the tool. */
export const PermissionRuleEffect = { Allow: "allow", Deny: "deny" } as const
export type PermissionRuleEffect =
  (typeof PermissionRuleEffect)[keyof typeof PermissionRuleEffect]
/** One remembered rule: a tool in one workspace, granted by the owner. */
export interface PermissionRule {
  /** Stable rule id. A replacement keeps it. */
  id: string
  /** Revision of this rule. The audit records the one that answered. */
  revision: string
  /** Workspace path the rule is scoped to. */
  workspace: string
  /** Harness the rule applies to. */
  source: "claude" | "codex"
  /** Tool name the rule answers. */
  tool: string
  /** Allow or deny. */
  effect: PermissionRuleEffect
}
/** List the owner's remembered permission rules. */
export interface ConversationPermissionRulesParams {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
}
/** The valid remembered rules, in the order they were stored. */
export interface ConversationPermissionRulesResult {
  /** Original action identifier. */
  requestId: string
  /** Rules the owner can revoke. */
  rules: PermissionRule[]
}
/** Revoke one remembered permission rule. */
export interface ConversationRevokePermissionRuleParams {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The rule to delete. */
  ruleId: string
}
/** Whether a rule with that id was deleted. */
export interface ConversationRevokePermissionRuleResult {
  /** Original action identifier. */
  requestId: string
  /** True when a rule was deleted. False when none had this id. */
  applied: boolean
}
/** One review waiting on a person. */
export interface PermissionReviewNotice {
  /** Conversation the review belongs to. */
  conversationId: string
  /** Execution that owns the review. */
  executionId: string
  /** Pending permission identity. */
  permissionId: string
  /** Tool the review named. */
  tool: string
  /** The conversation's name as conversation.list shows it, read when this set was published, so a notice can name a conversation the client has not listed yet. Omitted when none is on record. */
  title?: string
}
/** The reviews still waiting. A later event replaces this one; an empty list means none are waiting. */
export interface PermissionReviewWaiting {
  /** Waiting reviews, oldest first, at most 128. */
  reviews: PermissionReviewNotice[]
}
/** Committed semantic facts. A scheduling start is distinct from queued input; tool state is provider observation, not proof of physical effects; turn end is local settlement. */
export const ConversationEventKind = {
  TurnAccepted: "turn.accepted",
  TurnStarted: "turn.started",
  TurnEnded: "turn.ended",
  ToolUpdated: "tool.updated",
  StateChanged: "state.changed",
} as const
export type ConversationEventKind =
  (typeof ConversationEventKind)[keyof typeof ConversationEventKind]
/** Opaque SDK cursor bound to one conversation and stream incarnation. Save only after applying the batch. Reuse unchanged to resume; socket sequence numbers are not cursors. */
export type ConversationEventCursor = string
/** Metadata projected from one complete committed semantic fact. No prompt, thinking, tool arguments or results are included. */
export interface ConversationEvent {
  /** Stable semantic event identity, identical on replay. */
  id: ConversationEventCursor
  /** Opaque checkpoint after applying this event or batch. */
  cursor: ConversationEventCursor
  /** The semantic event kind. */
  kind: ConversationEventKind
  /** Execution this fact belongs to, when any. */
  executionId?: string
  /** Provider tool identity within its execution, when any. */
  toolId?: string
  /** Tool name when observed, bounded metadata only. */
  name?: string
  /** Observed lifecycle or tool status. */
  status?: string
  /** Local settlement outcome; this is not a provider acknowledgement. */
  outcome?:
    "completed" | "output_limit" | "request_limit" | "refused" | "cancelled" | "failed"
}
/** Follow committed semantic events without opening an Agent. Replay uses complete saved facts and explicit gap refusals; default is future events. Stop with conversation.unsubscribe. */
export interface ConversationSubscribeEventsParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Resume strictly after the last applied semantic cursor. */
  after?: ConversationEventCursor
  /** Start at the beginning when true. Omitted starts at the captured committed head. Mutually exclusive with after. */
  replay?: boolean
  /** Event kinds to deliver; omitted delivers all supported kinds. Filtered batches still advance their cursor. */
  kinds?: ConversationEventKind[]
}
/** An ordered bounded semantic event batch. An empty batch advances the checkpoint past filtered facts. Application checkpoints belong to the consumer. */
export interface ConversationEvents {
  /** The connection-local subscription this batch belongs to. */
  subscriptionId: ConversationSubscriptionId
  /** Opaque checkpoint after applying this event or batch. */
  cursor: ConversationEventCursor
  /** Committed semantic events in source order; empty advances filtered progress. */
  events: ConversationEvent[]
}
/** Change the model within the existing provider session, including while a turn runs. Effect timing follows the provider. */
export interface ConversationSetModelParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Exact model identifier within the conversation provider. */
  model: string
}
/** Carry the conversation on under another agent, in a new session of it that is given the conversation's earlier turns as text with its next message; the transcript shows a handoff line where it happened. Refused busy while a turn runs or waits; refused invalid_request for the agent the conversation already runs on (use conversation.setModel). applied is false when the switch was not made, and the conversation stays on its agent. */
export interface ConversationSwitchAgentParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The agent to carry on under, as agents.list names it. */
  agent: string
  /** Exact model identifier within that agent's catalog. */
  model: string
}
/** Start a new conversation from a copy of another's turns, on the same agent, model, approval preset and environment. The new conversation's agent is given the copied turns as text with its first message, and its transcript shows them first. Idempotent for a retry of one request; the conversation identity is the caller's, as for conversation.create. */
export interface ConversationForkParams {
  /** The new conversation, a fresh canonical lowercase hyphenated UUID. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** The conversation to copy, which must be the caller's own. */
  sourceConversationId: string
}
/** Durably confirmed model and approval preset, with native effect timing. */
export interface ConversationSetModelResult {
  /** Original action identifier. */
  requestId: string
  /** Exact model identifier within the conversation provider. */
  model: string
  /** Approval preset acknowledged with the model selection. */
  approvalMode: ApprovalMode
  /** Native boundary at which this selection takes effect. */
  effect: ModelChangeEffect
}
/** The native harness boundary at which acknowledged settings affect work. This does not relabel an already running turn. */
export const ModelChangeEffect = {
  SubsequentResponses: "subsequentResponses",
  NextTurn: "nextTurn",
} as const
export type ModelChangeEffect = (typeof ModelChangeEffect)[keyof typeof ModelChangeEffect]
/** A model change in progress or awaiting recovery; the last confirmed model remains separate. */
export interface ModelChange {
  /** Correlated action identifier. */
  requestId: string
  /** Turn admission state. */
  status: "changing" | "recovery_required"
  /** Exact model identifier within the conversation provider. */
  requestedModel: string
}
/** How the window lays out its workspace, as chosen in Settings: three columns (sidebar, session list, panes), or sessions listed in the sidebar beside the panes. */
export const WorkspaceArrangement = { Columns: "columns", Sidebar: "sidebar" } as const
export type WorkspaceArrangement =
  (typeof WorkspaceArrangement)[keyof typeof WorkspaceArrangement]
/** What fills the window's content region: the panes, the Agents overview drawn over them, or a widget drawn over them. */
export const WorkspaceShowing = {
  Panes: "panes",
  Agents: "agents",
  Widget: "widget",
} as const
export type WorkspaceShowing = (typeof WorkspaceShowing)[keyof typeof WorkspaceShowing]
/** A widget a pane or the window shows, by the plugin that draws it and its id within that plugin. */
export interface WorkspaceWidget {
  /** The plugin that draws the widget. */
  plugin: string
  /** The widget's id within its plugin. */
  id: string
}
/** What a pane shows: a conversation, a draft (a new conversation the person has not started yet), or a widget. Other kinds, such as an open file, are added here. */
export const WorkspacePaneKind = {
  Conversation: "conversation",
  Draft: "draft",
  Widget: "widget",
} as const
export type WorkspacePaneKind = (typeof WorkspacePaneKind)[keyof typeof WorkspacePaneKind]
/** What one pane shows. A conversation names its conversationId, a widget its widget, and a draft neither. */
export interface WorkspacePaneItem {
  /** What kind of thing the pane shows. */
  kind: WorkspacePaneKind
  /** Canonical lowercase hyphenated UUID of the conversation, when kind is conversation. */
  conversationId?: string
  /** The widget, when kind is widget. */
  widget?: WorkspaceWidget
}
/** One pane of a column, top to bottom. */
export interface WorkspacePane {
  /** The pane's share of its column's height, in whole percent. */
  heightPercent: number
  /** Whether this is the window's focused pane. At most one pane is. */
  focused: boolean
  /** What the pane shows. */
  item: WorkspacePaneItem
}
/** One column of panes, left to right. */
export interface WorkspaceColumn {
  /** The column's share of the panes' width, in whole percent. */
  widthPercent: number
  /** The column's panes, top to bottom. */
  panes: WorkspacePane[]
}
/** What one desktop window shows now: its arrangement, which side columns are drawn, what fills its content region, and its panes left to right, each column top to bottom. It names conversations by id only; their titles and agents are the gateway's to say. */
export interface WorkspaceWindow {
  /** The window's layout. */
  arrangement: WorkspaceArrangement
  /** Whether the sidebar is drawn: open by the person's choice and not folded for room. */
  sidebar: boolean
  /** Whether the session list column is drawn. The sidebar arrangement has none and always says false. */
  sessionList: boolean
  /** What fills the content region now. */
  showing: WorkspaceShowing
  /** The widget drawn over the panes, when showing is widget. */
  widget?: WorkspaceWidget
  /** The panes' columns, left to right. */
  columns: WorkspaceColumn[]
}
/** Why the gateway refused a channel method. invalid_request: a name, notes, path or model that is not one a channel holds (a relative path, or resources past x-channelRules.maxResourceBytes, among them), or an agent this gateway has no runtime for. forbidden: the access policy does not let the caller make or list channels; a paired device never may. conversations_not_configured: this gateway runs no conversations. channel_folder_not_found: the chosen folder does not exist. channel_folder_not_a_directory: the chosen folder is a file. channel_folder_refused: the chosen folder is one agents must not work in: the root of a drive, a system folder or one that holds a system folder, the home folder or one that holds it, or the gateway's own data folder, one that holds it, or anything inside it other than a folder under its channels folder. channel_resource_not_found: a file or folder to attach does not exist. channel_limit_reached: the caller keeps the most channels a person may (x-channelRules.maxChannels). channel_request_conflict: the requestId already made another channel; nothing was made. channel_storage_unavailable: the channel or its folder could not be saved or made; nothing was kept. temporarily_unavailable: the gateway is busy or stopping; ask again. */
export const ChannelErrorCode = {
  InvalidRequest: "invalid_request",
  Forbidden: "forbidden",
  ConversationsNotConfigured: "conversations_not_configured",
  ChannelFolderNotFound: "channel_folder_not_found",
  ChannelFolderNotADirectory: "channel_folder_not_a_directory",
  ChannelFolderRefused: "channel_folder_refused",
  ChannelResourceNotFound: "channel_resource_not_found",
  ChannelLimitReached: "channel_limit_reached",
  ChannelRequestConflict: "channel_request_conflict",
  ChannelStorageUnavailable: "channel_storage_unavailable",
  TemporarilyUnavailable: "temporarily_unavailable",
} as const
export type ChannelErrorCode = (typeof ChannelErrorCode)[keyof typeof ChannelErrorCode]
/** One channel as a list names it: where its conversations' agents work and what a new conversation in it starts with (agent, model, approval preset). Its notes and resources are told to its agents and are not listed. */
export interface Channel {
  /** The channel's id: default for the default channel, else ch_ and ten base32 characters the gateway chose. */
  channelId: string
  /** What the channel is called. */
  name: string
  /** The absolute folder its conversations' agents work in, resolved. */
  folder: string
  /** The gateway made the folder, under its channels folder, because none was chosen. False for a chosen folder and for the default channel's. */
  folderMade: boolean
  /** The agent a new conversation in the channel starts on; null for the default channel, whose new conversations start on whatever the surface picks. */
  agent: string | null
  /** The model a new conversation in the channel starts on; null for the default channel. */
  model: string | null
  /** The preset a new conversation in the channel starts with; ask for the default channel. */
  approvalMode: ApprovalMode
  /** When the channel was made, in Unix milliseconds; 0 for the default channel. */
  createdAtMs: number
  /** Where the channel coordinator should run, independently of worker threads. */
  coordinatorPlacement: "local" | "boat"
  /** Where new worker-thread workspaces run. Boat allocates one VM per thread. */
  threadPlacement: "local" | "boat"
  /** Stable main-agent conversation identity; null for the default channel. Opening a channel uses this conversation; New thread uses another identity. */
  coordinatorConversationId: string | null
}
/** Make a channel (channel.create). Repeating the same requestId with the same channel answers the channel it made; with another, channel_request_conflict. The folder is the one path a request may name, and only here: a conversation names its channel, never a folder. */
export interface ChannelCreateParams {
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** What the channel is called: trimmed, 1 to x-channelRules.maxNameChars characters, no control characters. */
  name: string
  /** An existing absolute folder the channel's agents work in. Omitted, the gateway makes one under its channels folder, named by the channel's id. */
  folder?: string
  /** The agent a new conversation in the channel starts on. */
  agent: string
  /** The model a new conversation in the channel starts on. */
  model: string
  /** The preset a new conversation in the channel starts with; default ask. */
  approvalMode?: ApprovalMode
  /** Notes every agent in the channel is told; omitted is none. */
  notes?: string
  /** Existing absolute files and folders to attach, none twice, at most x-channelRules.maxResourceBytes of paths in all; omitted is none. */
  resources?: string[]
  /** Where the channel coordinator should run, independently of worker threads. */
  coordinatorPlacement?: "local" | "boat"
  /** Where new worker-thread workspaces run. Boat allocates one VM per thread. */
  threadPlacement?: "local" | "boat"
}
/** The channel channel.create made, or made earlier under the same requestId. */
export interface ChannelCreateResult {
  channel: Channel
}
/** Which page of the caller's channels to list (channel.list). Empty for the first. */
export interface ChannelListParams {
  /** The cursor an earlier page returned: list the channels made after that one. An id the caller has no channel under is refused invalid_request. */
  after?: string
}
/** One page of the caller's channels (channel.list): the default channel first on the first page, then the rest in the order they were made. */
export interface ChannelListResult {
  /** This page's channels, as many as fit one frame. Only the first page holds the default channel, and only when the gateway has a workspace. */
  channels: Channel[]
  /** False when the frame bound left some out: the later ones made, listed from cursor. */
  complete: boolean
  /** Present exactly when complete is false: the last channel on this page, to send back as after for the next one. */
  cursor?: string
}
/** Tell the gateway what this connection's desktop window shows now (workspace.publish), so the person's agents can read it with the read_window tool. Each publication replaces this connection's previous one; one with a lower revision than the one held is not applied. A publication with no window withdraws it. The gateway keeps a window only while its connection is open: a closed connection withdraws it, and a window that connects again publishes again, under the same windowId, which replaces whatever an older connection of that window still holds. */
export interface WorkspacePublishParams {
  /** The window's own id, made once when it opens and kept across its connections, so a connection it left behind never reads as a second window. */
  windowId: string
  /** The window's publication number, increasing with each change and kept across its connections. */
  revision: number
  /** What the window shows. Absent withdraws the window: it shows no workspace now. */
  window?: WorkspaceWindow
}
/** The answer to workspace.publish: the revision the gateway holds for this connection now, and whether this publication was applied. */
export interface WorkspacePublishResult {
  /** The revision held now: this one when applied, a later one when this one arrived after it. */
  revision: number
  /** False when a later revision was already held, so this one changed nothing. */
  applied: boolean
}
/** The local vault state. Unlock belongs to the connection that proved its passphrase; closing its owner locks the vault. */
export const VaultState = {
  Unconfigured: "unconfigured",
  Locked: "locked",
  Unlocked: "unlocked",
} as const
export type VaultState = (typeof VaultState)[keyof typeof VaultState]
/** The current state of the local encrypted vault; never includes secret contents. */
export interface VaultStatus {
  /** Whether protected local credentials are unconfigured, locked, or available to this app session. */
  state: VaultState
}
/** Transient passphrase proof for creating or unlocking the local vault. Mutations must never be automatically replayed. */
export interface VaultPassphraseParams {
  /** Transient secret input, never logged or saved. Strength and byte bounds are owned by defaults/vault-contract.json. */
  passphrase: string
}
/** Redacted local-vault failures. Wrong passphrase can also mean damaged authenticated ciphertext; no failure resets the vault. */
export const VaultErrorCode = {
  VaultUnavailable: "vault_unavailable",
  VaultLocked: "vault_locked",
  VaultPassphrasePolicy: "vault_passphrase_policy",
  VaultWrongPassphrase: "vault_wrong_passphrase",
  VaultBusy: "vault_busy",
  VaultCooldown: "vault_cooldown",
  VaultSessionClosed: "vault_session_closed",
  VaultSessionOwned: "vault_session_owned",
  VaultAlreadyConfigured: "vault_already_configured",
  VaultCapacity: "vault_capacity",
  VaultAuditUnavailable: "vault_audit_unavailable",
  VaultMigrationUnavailable: "vault_migration_unavailable",
} as const
export type VaultErrorCode = (typeof VaultErrorCode)[keyof typeof VaultErrorCode]
/** Connect a user-owned Boat account. The gateway validates the token and its release worker image, configures its dedicated agent environment, then encrypts the token in the unlocked vault. Does not authenticate a model subscription. */
export interface BoatSaveParams {
  /** Boat infrastructure token. Never returned or logged. */
  token: string
}
/** Typed Boat connection refusals. Worker unavailable requires a matching Nessa release image; environment blocks credentials requires allowing connected agents in the dedicated Boat environment. Provisioning and cleanup uncertainty require reconciliation, not automatic replacement. No code or diagnostic contains credential values. */
export const BoatErrorCode = {
  VaultLocked: "vault_locked",
  BoatNotConfigured: "boat_not_configured",
  InvalidRequest: "invalid_request",
  BoatStorageUnavailable: "boat_storage_unavailable",
  BoatResourceMissing: "boat_resource_missing",
  BoatUnreachable: "boat_unreachable",
  BoatWorkerUnavailable: "boat_worker_unavailable",
  BoatEnvironmentBlocksCredentials: "boat_environment_blocks_credentials",
  BoatProvisioningUncertain: "boat_provisioning_uncertain",
  BoatCleanupUncertain: "boat_cleanup_uncertain",
} as const
export type BoatErrorCode = (typeof BoatErrorCode)[keyof typeof BoatErrorCode]
/** Secret-free saved Boat connection state. Connected means validation succeeded when saved; it does not attest a current model turn or revoke credentials in existing VMs. */
export interface BoatStatus {
  /** Locked when the vault cannot be read, disconnected when no connection is saved, or connected after validation and encrypted persistence. */
  state: "locked" | "disconnected" | "connected"
  /** The dedicated Boat environment saved with this connection; present only while connected. */
  environment?: string
  /** The release-owned shared worker image saved with this connection; present only while connected. */
  image?: string
}
/** Disable automatic stop or stop after a positive duration, including active work. */
export const VmLifetimeMode = { Never: "never", After: "after" } as const
export type VmLifetimeMode = (typeof VmLifetimeMode)[keyof typeof VmLifetimeMode]
/** Positive whole seconds representable by the Boat provider. */
export type VmLifetimeSeconds = number
/** The never mode omits seconds; after requires seconds. Validated together before any configuration write. */
export interface BoatVmLifetime {
  /** Disable automatic stop or use the finite seconds value. */
  mode: VmLifetimeMode
  /** Present only in after mode. Includes time spent on active work. */
  seconds?: VmLifetimeSeconds
}
/** Use the default (or reset the global initial choice), disable auto-stop, or choose a finite lifetime. */
export const BoatRuntimeSelection = {
  Default: "default",
  Never: "never",
  After: "after",
} as const
export type BoatRuntimeSelection =
  (typeof BoatRuntimeSelection)[keyof typeof BoatRuntimeSelection]
/** Read default and optional channel override from one ordinary configuration snapshot. Does not read a Boat token. */
export interface BoatRuntimeReadParams {
  /** Stored channel identity; omitted selects the Nessa default. The synthetic default channel inherits global settings. */
  channelId?: string
}
/** Change ordinary lifetime settings without a token. Default/never forbid seconds; after requires it. New allocations and archived-VM resumes use the saved choice; active VM deadlines are unchanged. Read settings after an uncertain response; calls are not replayed. */
export interface BoatRuntimeSetParams {
  /** Stored channel identity; omitted selects the Nessa default. The synthetic default channel inherits global settings. */
  channelId?: string
  /** Default removes the channel override or resets the global initial choice. Never disables automatic stop; after requires seconds. */
  mode: BoatRuntimeSelection
  /** Positive provider-compatible duration, required only in after mode. */
  seconds?: VmLifetimeSeconds
}
/** Non-secret settings. An omitted override inherits the default; explicit never remains unlimited when the default changes. Initial default is no automatic stop; provider/account limits still apply. */
export interface BoatRuntimeSettings {
  /** The Nessa default used by inheriting channels; initially no automatic stop. */
  defaultLifetime: BoatVmLifetime
  /** The requested channel's explicit choice. Omission inherits the default; never remains unlimited when the default changes. */
  channelOverride?: BoatVmLifetime
  /** The stored channel whose settings were read or saved. Omitted only for the global default; an override requires this identity. */
  channelId?: string
}
/** Read the owner-only registered webhook checkpoint. Performs no canonical record I/O or delivery control. */
export interface ConversationWebhookCursorParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The actual registered webhook target credential owned by this conversation owner; this is not a paired-device selector. */
  credentialId: string
  /** Explicitly request a separately observed canonical Head under current owner read authority. Omitted/false performs no canonical admission or physical Head I/O. This does not reconcile or mutate the native checkpoint. */
  includeSourceHead?: boolean
}
/** Reset only a frozen webhook checkpoint. requestId is its server-issued frozen capture UUID. Fresh owner authorization applies on replay; no caller-supplied scope or offset is accepted. */
export interface ConversationWebhookResetParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** The actual registered webhook target credential owned by this conversation owner; this is not a paired-device selector. */
  credentialId: string
  /** The frozen capture UUID returned by the owner cursor query. Explicit retries retain this exact identity. */
  requestId: string
}
/** Current native scoped checkpoint, without reconciliation or publication. An explicitly requested observedSourceHead is read afterward under current owner authority; compare complete scopes before positions. requestId is available only while reset-required. Neither observation grants receiver or send authority. */
export interface ConversationWebhookCursorResult {
  /** Complete scope/incarnation bound to the native checkpoint; its presence grants no read or delivery authority. */
  scope: RecordScope
  /** Canonical unsigned decimal position committed in this native scope. */
  position: string
  /** Whether fresh source capture requires an explicit owner reset for this frozen generation. */
  resetRequired: boolean
  /** Frozen capture UUID when resetRequired is true; null otherwise. Not a caller-selected rewind identity. */
  requestId: string | null
  /** Present only when explicitly requested and successfully read. Observed after the native checkpoint; these are separate non-atomic observations. Not a delivery, reset, publication or current-at-commit acknowledgement. */
  observedSourceHead?: ConversationRecordsHeadResult
}
/** Immutable result: reset applied for this explicitly observed scope at position zero. Does not promise current-at-commit, resumed delivery or current resetRequired status. Same request replays this result without another rewind. */
export interface ConversationWebhookResetResult {
  /** Original frozen capture request UUID; matching replay returns the same selection without another rewind. */
  requestId: string
  /** Immutable native selection and audit receipt UUID for this applied reset. */
  receiptId: string
  /** Complete server-observed incarnation selected by this reset. It need not remain current when the native commit completes. */
  scope: RecordScope
  /** Scoped checkpoint selected by the reset, always zero in the returned observed incarnation. */
  position: "0"
  /** Original native selection observation time in Unix milliseconds, retained unchanged on replay. */
  appliedAtMs: number
}
export const vmLifetimeSeconds = { minimum: 1, maximum: 4294967295 } as const
export const subscriptionLimits = {
  conversationTargets: 8,
  listTargets: 2,
  deliveryTimeoutMs: 10000,
} as const
export const conversationSubscriptionIdPattern = "^[1-9][0-9]{0,19}$" as const
export const maxViewCursorIncarnationLength = 128 as const
export const transcriptStates = [
  "not_loaded",
  "partial",
  "complete_empty",
  "complete",
  "stale",
  "unknown",
] as const
/** Passive source and delivery deadlines, plus the client allowance. The minimum request deadline is their sum; clients raise shorter configured timeouts to this floor. */
export const passiveReadTiming = {
  readTimeoutMs: 10000,
  deliveryTimeoutMs: 30000,
  clientAllowanceMs: 5000,
  minRequestTimeoutMs: 45000,
} as const
/** How long an MCP App's calls can take the gateway: a destructive tool's review waits up to reviewDeadlineMs for the person, then the call itself up to callTimeoutMs; a resource read up to readTimeoutMs; clientAllowanceMs covers audit writes, the response and scheduling. callDeadlineMs is reviewDeadlineMs + callTimeoutMs + clientAllowanceMs. A client waits at least callDeadlineMs for mcp.callTool and mcp.sendMessage, each of which can wait on a review, and for mcp.readResource and mcp.updateModelContext too, whose own steps fit within it; giving up sooner would drop an answer the gateway may still send. These bound a request to an open conversation. A closed one is opened first, the agent's launch and startup, and only then does the request's own budget, a review's among them, begin. No published deadline covers that opening, so a client that gives up after callDeadlineMs may still drop an answer the gateway sends later. */
export const mcpAppCallTiming = {
  reviewDeadlineMs: 300000,
  callTimeoutMs: 60000,
  readTimeoutMs: 10000,
  clientAllowanceMs: 10000,
  callDeadlineMs: 370000,
} as const
/** How mcpServers.inspect is bounded: one inspection runs at most deadlineMs, reads at most maxToolPages pages of tools and maxUiReads UI resources, and at most maxConcurrent run at once. The client waits requestDeadlineMs, the deadline plus clientAllowanceMs for stopping the server, the audit records and the response. */
export const mcpServerInspect = {
  deadlineMs: 30000,
  maxToolPages: 8,
  maxUiReads: 32,
  maxConcurrent: 2,
  clientAllowanceMs: 10000,
  requestDeadlineMs: 40000,
} as const
/** The SDK's rules for a stored MCP server, as x-mcpServerRules publishes them: at most maxServers servers, the managed one included; a name of 1 to nameMaxBytes bytes; at most maxArgs arguments of at most argMaxBytes bytes each; a variable name of 1 to environmentNameMaxBytes bytes. The gateway refuses past them (mcp_servers_invalid); a client may refuse early by reading these. */
export const mcpServerRules = {
  maxServers: 16,
  nameMaxBytes: 64,
  maxArgs: 64,
  argMaxBytes: 8192,
  environmentNameMaxBytes: 256,
} as const
/** A channel's bounds, as x-channelRules publishes them: at most maxChannels per person; a name of 1 to maxNameChars characters once trimmed; notes of at most maxNotesBytes bytes; at most maxResources files and folders of at most maxResourceBytes bytes of paths in all; each path at most maxPathBytes bytes. The gateway refuses past them (invalid_request, channel_limit_reached); a client may refuse early by reading these. */
export const channelRules = {
  maxChannels: 1000,
  maxNameChars: 80,
  maxNotesBytes: 8192,
  maxResources: 32,
  maxResourceBytes: 16384,
  maxPathBytes: 4096,
} as const
/** Bounds the product schema puts on attachments and conversations, generated from it so no copy of a number can drift. */
export const bounds = {
  maxOrdinaryResponseBytes: 65536,
  maxRequestFrameBytes: 65536,
  maxReadyMethods: 104,
  maxAuthCredentialCharacters: 16384,
  maxProductClientIdCharacters: 256,
  maxProductSurfaceInstanceCharacters: 256,
  maxPhysicalRecordPayloadBytes: 65546,
  maxRecordPageRecords: 16,
  maxRecordPagePayloadBytes: 65546,
  maxRecordResponseBytes: 131072,
  minAgentInstallRequestIdCharacters: 1,
  maxAgentInstallRequestIdBytes: 256,
  minNativeHooksRequestIdCharacters: 1,
  maxNativeHooksRequestIdBytes: 256,
  maxConfiguredAgents: 3,
  maxToolPolicyPatterns: 16,
  maxToolPatternBytes: 48,
  maxCommandHosts: 16,
  maxCommandHostBytes: 253,
  maxShareGrants: 64,
  maxSharePage: 8,
  maxAgentInstallVersionBytes: 128,
  maxImageBytes: 5242880,
  imageMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  maxMessageImages: 10,
  maxMessageImageBytes: 10485760,
  maxUploadBytes: 67108864,
  maxUploadedFileNameBytes: 255,
  uploadedFileNamePattern: "^[^/\\\\\\u0000-\\u001f\\u007f-\\u009f]+$",
  maxMessageFiles: 10,
  maxFilePathBytes: 4096,
  filePathPattern:
    "^(?:/(?!\\.{1,2}(?:/|$))[^/\\u0000-\\u001f\\u007f\\u0080-\\u009f]+)+$",
  maxBackgroundTaskIdBytes: 128,
  backgroundTaskIdPattern: "^[A-Za-z0-9_-]+$",
  conversationIdPattern: "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  maxConversationTitleBytes: 256,
  maxConversationRenameCharacters: 48,
  maxConversationPreviewBytes: 512,
  maxSyncIdBytes: 128,
  decimalU64Pattern: "^(0|[1-9][0-9]{0,19})$",
  maxDecimalU64Characters: 20,
  positiveEpochPattern: "^[1-9][0-9]{0,19}$",
  maxPositiveEpochCharacters: 20,
  recordPayloadPattern:
    "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$",
  minRecordPayloadEncodedCharacters: 4,
  maxRecordPayloadEncodedCharacters: 87396,
  maxListedConversations: 500,
  maxCataloguePageEntries: 256,
  maxToolStructuredContentBytes: 16384,
  maxMcpNameBytes: 128,
  maxUiResourceUriBytes: 2048,
  mcpAppInstanceIdPattern:
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  maxMcpArgumentsBytes: 32768,
  maxOpenReviews: 128,
  maxListedReviews: 144,
  maxReviewEntryToolNameBytes: 64,
  maxQuestionEntryMessageBytes: 48,
  maxOpenQuestions: 8,
  maxReviewInputBytes: 33554432,
  maxReviewOptions: 64,
  maxReviewOptionLabelBytes: 2048,
  maxReviewToolNameBytes: 256,
  maxReviewBytes: 268435456,
  maxReviewRangeBytes: 32768,
  maxMcpResultBytes: 57344,
  maxMcpMessageBytes: 8192,
  minMcpMessageCharacters: 1,
  maxExecutionIdBytes: 256,
  maxMcpContextBytes: 8192,
  maxMcpResourceUriBytes: 2048,
  mcpAppMimeType: "text/html;profile=mcp-app",
  maxMcpResourceBytes: 4194304,
  mcpResourceDigestPattern: "^[0-9a-f]{64}$",
  mcpResourceTicketPattern: "^[A-Za-z0-9_-]{43}$",
  mcpResourceTicketMs: 60000,
  maxMcpCspDomains: 64,
  maxMcpCspDomainBytes: 512,
  maxMcpDomainBytes: 512,
  maxMcpRemoteMessageCharacters: 512,
} as const
export const eventSubscriptionBounds = {
  maxCursorCharacters: 4096,
  maxBatchEvents: 64,
  minKinds: 1,
  maxKinds: 5,
  uniqueKinds: true,
  maxExecutionIdCharacters: 256,
  maxToolIdCharacters: 256,
  maxNameCharacters: 1024,
  maxStatusCharacters: 256,
  maxOutcomeCharacters: 256,
} as const
/** Kind/metadata relationships derived from ConversationEvent's JSON Schema conditionals. */
export const conversationEventMetadataRules = [
  {
    kind: "turn.accepted",
    required: ["executionId", "status"],
    forbidden: ["toolId", "name", "outcome"],
    values: {
      status: ["accepted"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "turn.started",
    required: ["executionId", "status"],
    forbidden: ["toolId", "name", "outcome"],
    values: {
      status: ["running"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "turn.ended",
    required: ["executionId", "status", "outcome"],
    forbidden: ["toolId", "name"],
    values: {
      status: ["settled"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "tool.updated",
    required: ["executionId", "toolId"],
    forbidden: ["outcome"],
    values: {
      status: ["pending", "running", "completed", "failed"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "state.changed",
    required: ["status"],
    forbidden: ["executionId", "toolId", "name", "outcome"],
    values: {
      status: ["opened"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "state.changed",
    required: ["executionId", "status", "outcome"],
    forbidden: ["toolId", "name"],
    values: {
      status: ["settlement_updated"],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
  {
    kind: "state.changed",
    required: ["executionId", "status"],
    forbidden: ["toolId", "name", "outcome"],
    values: {
      status: [
        "queued",
        "injected",
        "settled",
        "cancelled",
        "waiting_permission",
        "permission_cancelled",
        "permission_declined",
        "permission_deadline_answered",
        "waiting_question",
        "question_closed",
        "stop_requested",
      ],
      outcome: [
        "completed",
        "output_limit",
        "request_limit",
        "refused",
        "cancelled",
        "failed",
      ],
    },
  },
] as const
export const ProductMethod = {
  SessionAuthenticate: "session.authenticate",
  AuthSession: "auth.session",
  ServerHealth: "server.health",
  ServerReach: "server.reach",
  CredentialIssue: "credential.issue",
  CredentialList: "credential.list",
  CredentialRevoke: "credential.revoke",
  ConversationCreate: "conversation.create",
  ConversationRead: "conversation.read",
  ConversationRecordsHead: "conversation.recordsHead",
  ConversationRecordsPage: "conversation.recordsPage",
  ConversationList: "conversation.list",
  ConversationObserve: "conversation.observe",
  ConversationSend: "conversation.send",
  ConversationSteer: "conversation.steer",
  ConversationRemove: "conversation.remove",
  ConversationStop: "conversation.stop",
  ConversationReceipt: "conversation.receipt",
  ConversationMessage: "conversation.message",
  ConversationMessageStatus: "conversation.messageStatus",
  ConversationAnswer: "conversation.answer",
  ConversationPermissionReview: "conversation.permissionReview",
  ConversationQuestionReview: "conversation.questionReview",
  ConversationAnswerQuestion: "conversation.answerQuestion",
  ConversationCancel: "conversation.cancel",
  ConversationClose: "conversation.close",
  ConversationArchive: "conversation.archive",
  ConversationUnarchive: "conversation.unarchive",
  ConversationRename: "conversation.rename",
  ConversationDelete: "conversation.delete",
  ConversationReorder: "conversation.reorder",
  ArtifactRead: "artifact.read",
  AttachmentBegin: "attachment.begin",
  AttachmentWrite: "attachment.write",
  AgentsList: "agents.list",
  AgentsSetNativeHooks: "agents.setNativeHooks",
  AgentsSubscribe: "agents.subscribe",
  AgentsUnsubscribe: "agents.unsubscribe",
  ConversationSetApprovalMode: "conversation.setApprovalMode",
  ConversationSetUsageLimitAutoContinue: "conversation.setUsageLimitAutoContinue",
  AgentsInstallOptions: "agents.installOptions",
  AgentsInstall: "agents.install",
  ConversationCatalogueHead: "conversation.catalogueHead",
  ConversationCatalogueManifest: "conversation.catalogueManifest",
  ConversationCatalogueResolve: "conversation.catalogueResolve",
  McpCallTool: "mcp.callTool",
  McpReadResource: "mcp.readResource",
  McpReleaseApp: "mcp.releaseApp",
  McpSendMessage: "mcp.sendMessage",
  McpUpdateModelContext: "mcp.updateModelContext",
  McpServersList: "mcpServers.list",
  McpServersSave: "mcpServers.save",
  McpServersRemove: "mcpServers.remove",
  McpServersInspect: "mcpServers.inspect",
  McpServersAuthorize: "mcpServers.authorize",
  McpServersRevoke: "mcpServers.revoke",
  PhoneEndpoint: "phone.endpoint",
  PhoneAuthorize: "phone.authorize",
  PhoneStatus: "phone.status",
  PairingCreate: "pairing.create",
  PairingPending: "pairing.pending",
  PairingStatus: "pairing.status",
  PairingApprove: "pairing.approve",
  PairingDeny: "pairing.deny",
  PairingCancel: "pairing.cancel",
  PairingSetLinking: "pairing.setLinking",
  PairingSetTunnel: "pairing.setTunnel",
  PairingTunnel: "pairing.tunnel",
  GithubLiveEvents: "github.liveEvents",
  GithubStatus: "github.status",
  GithubConnect: "github.connect",
  GithubCancel: "github.cancel",
  GithubDisconnect: "github.disconnect",
  PeerEnroll: "peer.enroll",
  PeerList: "peer.list",
  PeerForget: "peer.forget",
  PeerRelay: "peer.relay",
  ConversationSubscribe: "conversation.subscribe",
  ConversationSubscribeList: "conversation.subscribeList",
  ConversationUnsubscribe: "conversation.unsubscribe",
  ConversationShare: "conversation.share",
  ConversationUnshare: "conversation.unshare",
  ConversationShares: "conversation.shares",
  ConversationPermissionRules: "conversation.permissionRules",
  ConversationRevokePermissionRule: "conversation.revokePermissionRule",
  WorkspacePublish: "workspace.publish",
  ChannelCreate: "channel.create",
  ChannelList: "channel.list",
  ConversationSubscribeEvents: "conversation.subscribeEvents",
  ConversationReadEarlier: "conversation.readEarlier",
  ConversationPeek: "conversation.peek",
  ConversationSetModel: "conversation.setModel",
  ConversationSwitchAgent: "conversation.switchAgent",
  ConversationFork: "conversation.fork",
  VaultStatus: "vault.status",
  VaultSetup: "vault.setup",
  VaultUnlock: "vault.unlock",
  VaultLock: "vault.lock",
  BoatStatus: "boat.status",
  BoatSave: "boat.save",
  BoatDisconnect: "boat.disconnect",
  BoatRuntimeSettings: "boat.runtimeSettings",
  BoatSetRuntimeSettings: "boat.setRuntimeSettings",
  ConversationWebhookCursor: "conversation.webhook.cursor",
  ConversationWebhookReset: "conversation.webhook.reset",
} as const
export const ProductEvent = {
  SessionChallenge: "session.challenge",
  SessionTerminated: "session.terminated",
  ConversationView: "conversation.view",
  ConversationListed: "conversation.listed",
  AgentsListed: "agents.listed",
  ConversationSubscriptionEnded: "conversation.subscriptionEnded",
  PermissionReviewWaiting: "permission.reviewWaiting",
  ConversationEvents: "conversation.events",
} as const
export const ProductHandshakeMethod = "session.authenticate" as const
export const productReadyMethods = [
  "auth.session",
  "server.health",
  "server.reach",
  "credential.issue",
  "credential.list",
  "credential.revoke",
  "conversation.create",
  "conversation.read",
  "conversation.recordsHead",
  "conversation.recordsPage",
  "conversation.list",
  "conversation.observe",
  "conversation.send",
  "conversation.steer",
  "conversation.remove",
  "conversation.stop",
  "conversation.receipt",
  "conversation.message",
  "conversation.messageStatus",
  "conversation.answer",
  "conversation.permissionReview",
  "conversation.questionReview",
  "conversation.answerQuestion",
  "conversation.cancel",
  "conversation.close",
  "conversation.archive",
  "conversation.unarchive",
  "conversation.rename",
  "conversation.delete",
  "conversation.reorder",
  "artifact.read",
  "attachment.begin",
  "attachment.write",
  "agents.list",
  "agents.setNativeHooks",
  "agents.subscribe",
  "agents.unsubscribe",
  "conversation.setApprovalMode",
  "conversation.setUsageLimitAutoContinue",
  "agents.installOptions",
  "agents.install",
  "conversation.catalogueHead",
  "conversation.catalogueManifest",
  "conversation.catalogueResolve",
  "mcp.callTool",
  "mcp.readResource",
  "mcp.releaseApp",
  "mcp.sendMessage",
  "mcp.updateModelContext",
  "mcpServers.list",
  "mcpServers.save",
  "mcpServers.remove",
  "mcpServers.inspect",
  "mcpServers.authorize",
  "mcpServers.revoke",
  "phone.endpoint",
  "phone.authorize",
  "phone.status",
  "pairing.create",
  "pairing.pending",
  "pairing.status",
  "pairing.approve",
  "pairing.deny",
  "pairing.cancel",
  "pairing.setLinking",
  "pairing.setTunnel",
  "pairing.tunnel",
  "github.liveEvents",
  "github.status",
  "github.connect",
  "github.cancel",
  "github.disconnect",
  "peer.enroll",
  "peer.list",
  "peer.forget",
  "peer.relay",
  "conversation.subscribe",
  "conversation.subscribeList",
  "conversation.unsubscribe",
  "conversation.share",
  "conversation.unshare",
  "conversation.shares",
  "conversation.permissionRules",
  "conversation.revokePermissionRule",
  "workspace.publish",
  "channel.create",
  "channel.list",
  "conversation.subscribeEvents",
  "conversation.readEarlier",
  "conversation.peek",
  "conversation.setModel",
  "conversation.switchAgent",
  "conversation.fork",
  "vault.status",
  "vault.setup",
  "vault.unlock",
  "vault.lock",
  "boat.status",
  "boat.save",
  "boat.disconnect",
  "boat.runtimeSettings",
  "boat.setRuntimeSettings",
  "conversation.webhook.cursor",
  "conversation.webhook.reset",
] as const
/** The grant each product method asks Cedar for, generated from protocol/product/manifest.json. null means another owner admits the method: the handshake or auth.session. Writing to a conversation — an answer, an upload, an app's calls — asks for conversation.write; reading its records or catalogue asks for conversation.read; running a configured server, or enrolling a device, asks for credential.manage. */
export const productMethodGrants = {
  "session.authenticate": null,
  "auth.session": null,
  "server.health": "server.read",
  "server.reach": "server.read",
  "credential.issue": "credential.manage",
  "credential.list": "credential.manage",
  "credential.revoke": "credential.manage",
  "conversation.create": "conversation.write",
  "conversation.read": "conversation.write",
  "conversation.recordsHead": "conversation.read",
  "conversation.recordsPage": "conversation.read",
  "conversation.list": "conversation.write",
  "conversation.observe": "conversation.write",
  "conversation.send": "conversation.write",
  "conversation.steer": "conversation.write",
  "conversation.remove": "conversation.write",
  "conversation.stop": "conversation.write",
  "conversation.receipt": "conversation.write",
  "conversation.message": "conversation.write",
  "conversation.messageStatus": "conversation.write",
  "conversation.answer": "conversation.write",
  "conversation.permissionReview": "conversation.write",
  "conversation.questionReview": "conversation.write",
  "conversation.answerQuestion": "conversation.write",
  "conversation.cancel": "conversation.write",
  "conversation.close": "conversation.write",
  "conversation.archive": "conversation.write",
  "conversation.unarchive": "conversation.write",
  "conversation.rename": "conversation.write",
  "conversation.delete": "conversation.write",
  "conversation.reorder": "conversation.write",
  "artifact.read": "conversation.read",
  "attachment.begin": "conversation.write",
  "attachment.write": "conversation.write",
  "agents.list": "server.read",
  "agents.setNativeHooks": "credential.manage",
  "agents.subscribe": "server.read",
  "agents.unsubscribe": "server.read",
  "conversation.setApprovalMode": "conversation.write",
  "conversation.setUsageLimitAutoContinue": "credential.manage",
  "agents.installOptions": "server.read",
  "agents.install": "conversation.write",
  "conversation.catalogueHead": "conversation.read",
  "conversation.catalogueManifest": "conversation.read",
  "conversation.catalogueResolve": "conversation.read",
  "mcp.callTool": "conversation.write",
  "mcp.readResource": "conversation.write",
  "mcp.releaseApp": "conversation.write",
  "mcp.sendMessage": "conversation.write",
  "mcp.updateModelContext": "conversation.write",
  "mcpServers.list": "credential.manage",
  "mcpServers.save": "credential.manage",
  "mcpServers.remove": "credential.manage",
  "mcpServers.inspect": "credential.manage",
  "mcpServers.authorize": "credential.manage",
  "mcpServers.revoke": "credential.manage",
  "phone.endpoint": "server.read",
  "phone.authorize": "credential.manage",
  "phone.status": "credential.manage",
  "pairing.create": "credential.manage",
  "pairing.pending": "credential.manage",
  "pairing.status": "credential.manage",
  "pairing.approve": "credential.manage",
  "pairing.deny": "credential.manage",
  "pairing.cancel": "credential.manage",
  "pairing.setLinking": "credential.manage",
  "pairing.setTunnel": "credential.manage",
  "pairing.tunnel": "credential.manage",
  "github.liveEvents": "credential.manage",
  "github.status": "credential.manage",
  "github.connect": "credential.manage",
  "github.cancel": "credential.manage",
  "github.disconnect": "credential.manage",
  "peer.enroll": "credential.manage",
  "peer.list": "credential.manage",
  "peer.forget": "credential.manage",
  "peer.relay": "credential.manage",
  "conversation.subscribe": "conversation.write",
  "conversation.subscribeList": "conversation.write",
  "conversation.unsubscribe": "conversation.write",
  "conversation.share": "credential.manage",
  "conversation.unshare": "credential.manage",
  "conversation.shares": "credential.manage",
  "conversation.permissionRules": "conversation.write",
  "conversation.revokePermissionRule": "conversation.write",
  "workspace.publish": "conversation.write",
  "channel.create": "conversation.write",
  "channel.list": "conversation.write",
  "conversation.subscribeEvents": "conversation.write",
  "conversation.readEarlier": "conversation.write",
  "conversation.peek": "conversation.write",
  "conversation.setModel": "conversation.write",
  "conversation.switchAgent": "conversation.write",
  "conversation.fork": "conversation.write",
  "vault.status": "credential.manage",
  "vault.setup": "credential.manage",
  "vault.unlock": "credential.manage",
  "vault.lock": "credential.manage",
  "boat.status": "credential.manage",
  "boat.save": "credential.manage",
  "boat.disconnect": "credential.manage",
  "boat.runtimeSettings": "credential.manage",
  "boat.setRuntimeSettings": "credential.manage",
  "conversation.webhook.cursor": "credential.manage",
  "conversation.webhook.reset": "credential.manage",
} as const
export const catalogueWireSchemas = {
  RecordScope: {
    type: "object",
    additionalProperties: false,
    properties: {
      receiver: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      origin: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      stream: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      incarnation: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      schema: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      accessEpoch: {
        type: "string",
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
    },
    required: ["receiver", "origin", "stream", "incarnation", "schema", "accessEpoch"],
  },
  CatalogueEntryKey: {
    type: "object",
    additionalProperties: false,
    properties: {
      creation: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
      id: {
        type: "string",
        minLength: 1,
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
    },
    required: ["creation", "id"],
  },
  CatalogueDescriptor: {
    type: "object",
    additionalProperties: false,
    properties: {
      key: { $ref: "#/$defs/CatalogueEntryKey" },
      revision: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
      deleted: { type: "boolean" },
    },
    required: ["key", "revision", "deleted"],
  },
  CataloguePass: {
    type: "object",
    additionalProperties: false,
    properties: {
      scope: { $ref: "#/$defs/RecordScope" },
      completed: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
      boundary: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
      cursor: { $ref: "#/$defs/CatalogueEntryKey" },
      generation: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
    },
    required: ["scope", "completed", "boundary", "generation"],
  },
  CatalogueManifestRequest: {
    type: "object",
    additionalProperties: false,
    properties: {
      pass: { $ref: "#/$defs/CataloguePass" },
      maxEntries: {
        type: "integer",
        minimum: 1,
        "x-core-maximum-bound": "catalogue_max_entries",
        maximum: 256,
      },
    },
    required: ["pass", "maxEntries"],
  },
  ConversationCatalogueHeadParams: {
    type: "object",
    additionalProperties: false,
    properties: {
      receiverId: {
        type: "string",
        minLength: 1,
        "x-core-utf8-bound": "id_max_utf8_bytes",
        "x-utf8MaxBytes": 128,
      },
      accessEpoch: { type: "string", pattern: "^[1-9][0-9]{0,19}$", maxLength: 20 },
    },
    required: ["receiverId", "accessEpoch"],
  },
  ConversationCatalogueHeadResult: {
    type: "object",
    additionalProperties: false,
    properties: {
      scope: { $ref: "#/$defs/RecordScope" },
      head: {
        type: "string",
        minLength: 1,
        maxLength: 20,
        pattern: "^(0|[1-9][0-9]{0,19})$",
      },
    },
    required: ["scope", "head"],
    "x-maxEncodedBytes": 131072,
  },
  ConversationCatalogueManifestParams: {
    type: "object",
    additionalProperties: false,
    properties: {
      request: { $ref: "#/$defs/CatalogueManifestRequest" },
      accessEpoch: { type: "string", pattern: "^[1-9][0-9]{0,19}$", maxLength: 20 },
    },
    required: ["request", "accessEpoch"],
  },
  ConversationCatalogueManifestResult: {
    type: "object",
    additionalProperties: false,
    properties: {
      request: { $ref: "#/$defs/CatalogueManifestRequest" },
      entries: {
        type: "array",
        items: { $ref: "#/$defs/CatalogueDescriptor" },
        "x-core-max-items-bound": "catalogue_max_entries",
        maxItems: 256,
      },
      hasMore: { type: "boolean" },
    },
    required: ["request", "entries", "hasMore"],
    "x-maxEncodedBytes": 131072,
  },
  ConversationCatalogueResolveParams: {
    type: "object",
    additionalProperties: false,
    properties: {
      pass: { $ref: "#/$defs/CataloguePass" },
      descriptor: { $ref: "#/$defs/CatalogueDescriptor" },
      maxPayloadBytes: {
        type: "integer",
        minimum: 1,
        "x-core-maximum-bound": "catalogue_max_payload_bytes",
        maximum: 1048576,
      },
      accessEpoch: { type: "string", pattern: "^[1-9][0-9]{0,19}$", maxLength: 20 },
    },
    required: ["pass", "descriptor", "maxPayloadBytes", "accessEpoch"],
  },
  ConversationCatalogueResolveResult: {
    type: "object",
    additionalProperties: false,
    properties: {
      pass: { $ref: "#/$defs/CataloguePass" },
      descriptor: { $ref: "#/$defs/CatalogueDescriptor" },
      entry: { $ref: "#/$defs/CatalogueDescriptor" },
      payload: {
        type: "string",
        "x-core-base64-bound": "catalogue_max_payload_bytes",
        pattern: "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$",
        maxLength: 1398104,
      },
    },
    required: ["pass", "descriptor", "entry", "payload"],
    "x-maxEncodedBytes": 131072,
  },
  CatalogueReadErrorCode: {
    type: "string",
    enum: [
      "invalid_request",
      "unauthorized",
      "forbidden",
      "wrong_owner",
      "wrong_receiver",
      "stale_epoch",
      "unverifiable",
      "identity_changed",
      "source_unavailable",
      "oversized_entry",
      "read_timeout",
      "response_too_large",
      "server_busy",
    ],
  },
} as const
