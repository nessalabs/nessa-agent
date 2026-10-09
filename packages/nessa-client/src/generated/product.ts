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
  /** Authenticated human, integration, or agent identity. */
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
/** Identity that acts in the product: a human, integration, or agent. A principal needs an active organization membership and an appropriate credential to access the gateway. */
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
/** Supported categories of authenticated actors: human users, integrations, and agents. */
export const PrincipalKind = {
  Human: "human",
  Integration: "integration",
  Agent: "agent",
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
/** Whether Nessa supports an explicit nonterminal defer outcome for a permission review. */
export const PermissionDeferralSupport = {
  UnsupportedNotImplemented: "unsupported_not_implemented",
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
  UnsupportedNotImplemented: "unsupported_not_implemented",
  SupportedForCurrentInvocation: "supported_for_current_invocation",
} as const
export type PolicyEndTurnSupport =
  (typeof PolicyEndTurnSupport)[keyof typeof PolicyEndTurnSupport]
/** Whether a Nessa policy can close its correlated provider session. */
export const PolicyCloseSessionSupport = {
  Unknown: "unknown",
  UnsupportedNotImplemented: "unsupported_not_implemented",
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
/** One bounded conversation turn; omitted older text is indicated by the enclosing truncated flag. */
export interface ConversationMessage {
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** User input text; empty for a message of images alone. */
  userText: string
  /** Images the user sent with this turn, in attachment order. */
  attachments: ImageAttachment[]
  /** Files the user pointed this turn at, in attachment order. No bytes were ever carried for them. */
  files: LinkedFile[]
  /** The app that wrote this turn on the person's behalf; absent when the person wrote it. */
  app?: ConversationMessageApp
  /** Current invocation state. */
  status: ConversationMessageStatus
  /** Bounded diagnostic for this invocation. */
  error?: string
  /** The provider adapter explicitly requires authentication for this failed turn. Generic provider codes and diagnostic text do not establish this fact. */
  authenticationRequired?: boolean
  /** Execution that consumed this injected steering input; its shared reply answers this input. */
  steeringTarget?: string
  /** Provider observations in execution order; offsets address the retained SDK event sequence. */
  parts: ConversationPart[]
  /** Target event count when steering was admitted locally; does not imply provider consumption timing. */
  steeringOffset?: number
}
/** A waiting input that may be removed before dispatch. */
export interface ConversationPending {
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
  /** Waiting user input; empty for a message of images alone. */
  text: string
  /** Images waiting with this input, in attachment order. */
  attachments: ImageAttachment[]
  /** Files the waiting input points at, in attachment order. */
  files: LinkedFile[]
  /** The app that wrote this waiting input on the person's behalf; absent when the person wrote it. */
  app?: ConversationMessageApp
  /** Queue or steering admission. */
  mode: ConversationPendingMode
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
/** A pending review with its complete offered choices. */
export interface ConversationPermission {
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
  /** Exact original JSON input reviewed by the user; never truncated. */
  argumentsJson: string
}
/** Bounded presentation of an observed tool call. */
export interface ConversationTool {
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
  /** Exact tool arguments observed through a permission request, or empty when unavailable. */
  input: string
  /** The MCP server and tool the call went to, as the agent's harness named them (Claude's harness replaces characters outside [A-Za-z0-9_-] in a tool name with _); absent for a harness's own tools and where the harness does not say which server. */
  mcp?: ConversationMcpTool
  /** The call's structured result (MCP structuredContent) as JSON text, when the harness passed it on and it fits; absent otherwise. The result's text stays in details either way. */
  structuredContent?: string
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
  /** Recent turns with current streamed output. */
  messages: ConversationMessage[]
  /** Current removable queue entries. */
  pending: ConversationPending[]
  /** Actionable permission reviews. */
  permissions: ConversationPermission[]
  /** Recent tool states. */
  tools: ConversationTool[]
  /** Agent operation support. */
  capabilities: ConversationCapabilities
  /** Current provider attachment lifecycle. */
  lifecycle: ConversationLifecycle
  /** Some history, text or whole interactions were omitted to bound this response; interactionViewError explains omitted actionable units. */
  truncated: boolean
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
  /** Name the gateway derived from the conversation's first message, the same one conversation.list shows; null before anything was said. */
  title: string | null
  /** Questions the agent is waiting on, oldest first. */
  questions: ConversationQuestion[]
  /** Last durably committed preset. */
  approvalMode: ApprovalMode
  /** Presets supported by this model. */
  approvalModes: ApprovalModeChoice[]
  /** Present while a change is in progress or recovery is required. */
  approvalModeChange?: ApprovalModeChange
}
/** Idempotently create or reopen one named conversation. */
export interface ConversationCreateParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Coding agent this conversation runs on for its whole life. Omitted takes the gateway's configured default. Ignored when the conversation already exists, which is reopened on the agent it was created with. */
  agent?: string
  /** Catalog model identifier fixed at first creation; ignored on reopen. */
  model?: string
  /** Initial preset, default ask; ignored on reopen. */
  approvalMode?: ApprovalMode
}
/** Conversation ready for read and admission. */
export interface ConversationCreateResult {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
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
  /** Name the gateway derived from the conversation's first message, the same one its view carries; null when none is on record. */
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
/** What a settled submit or stop did. Absent until the stage is settled. Already final means nothing was sent. */
export const ConversationCommandOutcome = {
  Dispatched: "dispatched",
  Withdrawn: "withdrawn",
  Cancelled: "cancelled",
  AlreadyFinal: "already_final",
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
  /** Original submit or steer file paths, in attachment order. Omitted means an empty list, the same fingerprint as a send that carried none. */
  files?: LinkedFile[]
  /** Creation hint. Omitted when the original creation named no agent. */
  agent?: string
  /** Creation hint. Omitted when the original creation named no model. */
  model?: string
  /** Creation hint. Omitted when the original creation named no preset. */
  approvalMode?: ApprovalMode
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
  /** Pending permission identity. */
  permissionId: string
  /** Exact provider-offered option identity. */
  optionId: string
}
/** Cancel one permission review with a caller reason. */
export interface ConversationCancelParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Stable invocation identifier retained for retries of one logical message, at most 256 UTF-8 bytes. */
  executionId: string
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
/** Delete a conversation permanently: stop it, ask its agent to delete the agent's own session, record who deleted it, and erase its history, uploads and summary. Audit evidence is retained. The identity is never reused: its owner's later commands on it answer conversation_deleted, except deleting it again, and anyone else is told conversation_not_found. A repeat of the deciding request — the same principal, surface and requestId — is answered applied true, any other delete by its owner applied false; either repeat first carries on an erasure that has not finished. conversation_erasure_incomplete and audit_unavailable mean the conversation is deleted and its erasure did not finish; deleting again, and each gateway start, tries again. Any other error from a delete means only that it is not known whether the conversation was deleted: list it, or delete again. */
export interface ConversationDeleteParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
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
  /** Where the agent runs. here: the gateway's own machine. */
  environment?: "here"
  /** The sandbox granted, or for a refused lease the one asked for. harness_default: whatever the agent's harness encloses by default, configured by nothing Nessa sets, and nothing more. */
  sandbox?: "harness_default"
  /** Why the lease is ending or ended: the first cause recorded. lost: the environment no longer had a process for it, for example because the gateway started again. */
  cause?: "stopped" | "closed" | "revoked" | "expired" | "lost"
  /** What the environment reported releasing, once it has: confirmed, released cooperatively; forced, released by forced termination; no_process, nothing was left to release. On an interrupted lease this is the evidence that arrived after its deadline. */
  cleanup?: "confirmed" | "forced" | "no_process"
  /** Why the lease was refused. sandbox_unavailable: the sandbox asked for is not one both the agent's binding and the environment can enforce. */
  refusal?: "sandbox_unavailable"
  /** Events that arrived after the lease ended and were dropped rather than applied. */
  droppedEvents: number
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
/** One ordered observation fragment in a bounded execution projection; offsets may have gaps. */
export interface ConversationPart {
  /** Zero-based SDK observation offset within the owning execution, used to preserve order. */
  offset: number
  /** Text, exposed thought content, a tool call, or a Nessa-owned runtime notice. A tool call is one part however many updates it has, at its first update's offset; its current state is its entry in tools. */
  kind: "text" | "thought" | "tool" | "local_notice"
  /** Exact text fragment for text, thought, or local notice observations; empty for a tool call. */
  text: string
  /** The tool call's identity, which names its entry in tools; empty otherwise. */
  toolId: string
  /** Stable execution-scoped declined-review identity for a local notice; empty otherwise. */
  noticeId: string
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
/** Typed rejection code carried by a conversation command the gateway dispatched and refused. Branch on these instead of message text. These are not every code a conversation request can receive: access and routing failures are answered by the session before a conversation command is dispatched, and carry their own codes. agent_startup_deadline means the agent was still starting when its budget expired, so nothing reached the provider and the same command is safe to repeat; it normally succeeds once the runtime is warm, but a launch whose process could not be confirmed stopped keeps that conversation blocked. invalid_request and agent_not_configured reject the command until their cause is addressed. agent_not_configured, agent_unsupported and conversations_not_configured are three different situations and only one of them is fixed by configuring an agent: the gateway runs no conversations at all, it names no runtime under the agent this conversation asked for, or no build here can open that conversation's agent. sandbox_unavailable means no lease could be granted to run the agent because the sandbox it asks for is not one both its agent binding and the place it would run can enforce; nothing ran, the refusal is in the conversation's records, and repeating the command fails the same way until that configuration changes. The image codes answer `attachment.begin` and a message naming uploads: image_input_unsupported is a model that takes no images, so no ticket and no message with one will ever be taken; attachment_not_found is an image this conversation does not hold — never uploaded into it, expired, or released when it closed; attachment_unavailable is one it holds but could not read; attachment_capacity is no room for another upload right now; attachment_storage_unavailable is the gateway unable to keep the bytes. attachment_cleanup_unavailable is a close that did happen, whose release of this conversation's uploads did not, and is the one image code that is not a refusal of the command. conversation_deleted refuses every command its owner sends on a conversation somebody deleted, except deleting it again; anyone else is told conversation_not_found. Its identity is never reused, so a surface still holding it should let it go. conversation_erasure_incomplete is a delete that did happen — the conversation is gone and every command on it is refused — whose erasure of stored data did not finish; repeating the delete, and each gateway start, tries again, but an agent that keeps refusing to delete its own session, or a damaged history, needs the operator. The mcp_ codes refuse an MCP App's request (mcp.callTool, mcp.readResource, mcp.sendMessage, mcp.updateModelContext): mcp_app_unknown, the app is not an MCP tool call with a UI in this conversation, or the resource is not an app's; mcp_server_mismatch, it names another server than the app's; mcp_tool_not_for_app, the tool is not listed with visibility including app; mcp_session_unavailable, the conversation has no open session of that server, or it ended; mcp_approval_denied and mcp_approval_expired, the person refused, or did not answer within x-mcpAppCallTiming.reviewDeadlineMs; mcp_cancelled, the review was withdrawn because the request was cancelled, the app was torn down, or the conversation ended, or, with no review withdrawn, the app's mount was released or the opening it was drawn in ended (closed, deleted, stopped, another begun, or the gateway stopping) before the request took effect, which for mcp.sendMessage sends nothing and opens nothing; mcp_request_too_large and mcp_result_too_large, past a request's bound (the arguments' 32 KiB, a message's input bound or its review's room, a context's 8 KiB) and a result's 56 KiB; turn_running also refuses an app's message while a turn runs or input waits; mcp_timed_out, the server did not answer in time; mcp_remote_error, the server answered with a JSON-RPC error (McpRemoteErrorDetails), or with something that is no MCP answer (no details). mcp_unauthorized, the remote endpoint refused the caller and the tool was not run; the answer carries no token. mcp_unreachable, the remote endpoint could not be reached and nothing was retained. mcp_insufficient_scope, the caller's scope was not enough and the call was not retried. These three are not mcp_session_unavailable: the session was not reported as ended. */
export const ConversationErrorCode = {
  AgentNotConfigured: "agent_not_configured",
  AgentUnsupported: "agent_unsupported",
  SandboxUnavailable: "sandbox_unavailable",
  ModelUnavailable: "model_unavailable",
  ApprovalModeUnavailable: "approval_mode_unavailable",
  ApprovalModeNotApplied: "approval_mode_not_applied",
  ApprovalModeUncertain: "approval_mode_uncertain",
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
/** A question the agent is waiting on an answer to. Nothing is authorised by answering; the agent simply cannot continue that path until it hears back. */
export interface ConversationQuestion {
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
/** Configured agents and their model-specific choices. */
export interface AgentsListResult {
  /** Configured agents. */
  agents: AgentOption[]
}
/** Change a conversation approval preset while no turn runs. */
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
  /** Current owner catalogue head. */
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
  /** Fixed class of access the consent is for. */
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
/** Why the gateway refused a pairing method it dispatched. pairing_not_configured: native pairing is off in this gateway's configuration. pairing_not_found: no such invitation. pairing_slot_occupied: an invitation is already open; cancel it first. pairing_capacity: too many unfinished enrollments. pairing_conflict: the request names a different key or outcome than the one recorded. pairing_ineligible: the enrollment cannot take this step now (expired, ended, or not yet claimed). pairing_busy: another create is running. pairing_unavailable: storage or a worker failed; retry later. */
export const PairingErrorCode = {
  PairingNotConfigured: "pairing_not_configured",
  PairingNotFound: "pairing_not_found",
  PairingSlotOccupied: "pairing_slot_occupied",
  PairingCapacity: "pairing_capacity",
  PairingConflict: "pairing_conflict",
  PairingIneligible: "pairing_ineligible",
  PairingBusy: "pairing_busy",
  PairingUnavailable: "pairing_unavailable",
} as const
export type PairingErrorCode = (typeof PairingErrorCode)[keyof typeof PairingErrorCode]
/** Opaque current-connection watch identity. It grants no permission, is not a source position, and must be discarded on connection replacement. */
export type ChangeWatchId = string
/** Register catalogue interest before the final authorized head recheck. Acknowledgement precedes notices; no head or source read is performed. */
export interface ConversationWatchCatalogueParams {
  /** Server-bound receiver identity requesting this owner catalogue. */
  receiverId: string
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
}
/** Register stable conversation interest before final head recheck. Reset notices do not carry a source incarnation. */
export interface ConversationWatchRecordsParams {
  /** Conversation selected under authenticated ownership. */
  conversationId: string
  /** Trusted receiver binding selector; the actual physical scope is returned by the authenticated head read. */
  receiverId: string
  /** Positive current numeric receiver binding epoch. */
  accessEpoch: string
}
/** A physically delivered acknowledgement activates this connection-local registration. */
export interface ConversationWatchResult {
  /** Opaque identity minted for this physical connection after producer installation; retain it only for this connection. */
  watchId: ChangeWatchId
}
/** Remove only this connection’s watch interest. Repeating an allocated identity is idempotent. */
export interface ConversationUnwatchParams {
  /** Exact identity minted on this connection whose source interest is to be removed. */
  watchId: ChangeWatchId
}
/** Removal acknowledgement. An already-started frame may finish before this response; no subsequent hint is admitted. */
export interface ConversationUnwatchResult {
  /** Original connection identity echoed after interest removal; previously admitted authority or physical frame work may remain. */
  watchId: ChangeWatchId
}
/** Advisory payloadless dirty notice. Reauthorize and recheck heads; never advance progress from this event. */
export interface ConversationChanged {
  /** Opaque connection identity of the advisory hint; this field carries no source progress or authority. */
  watchId: ChangeWatchId
}
/** Terminal advisory producer state. Neither outcome proves current source freshness or lost permission. */
export const ChangeWatchEndReason = {
  Closed: "closed",
  NotificationFailed: "notification_failed",
} as const
export type ChangeWatchEndReason =
  (typeof ChangeWatchEndReason)[keyof typeof ChangeWatchEndReason]
/** Terminal producer notice under the same bounded delivery owner as changed notices. Recover with explicit reads/fallback. */
export interface ConversationWatchEnded {
  /** Opaque connection identity of the producer interest that ended. */
  watchId: ChangeWatchId
  /** Typed terminal producer outcome; neither value proves source freshness or permission revocation. */
  reason: ChangeWatchEndReason
}
/** Typed watch admission/refusal. Watch state never changes downloaded/applied progress. */
export const ChangeWatchErrorCode = {
  InvalidRequest: "invalid_request",
  Unauthorized: "unauthorized",
  Forbidden: "forbidden",
  WrongOwner: "wrong_owner",
  WrongReceiver: "wrong_receiver",
  StaleEpoch: "stale_epoch",
  Unverifiable: "unverifiable",
  TemporarilyUnavailable: "temporarily_unavailable",
  WatchDuplicate: "watch_duplicate",
  WatchCapacity: "watch_capacity",
  WatchClosed: "watch_closed",
  InvalidWatch: "invalid_watch",
} as const
export type ChangeWatchErrorCode =
  (typeof ChangeWatchErrorCode)[keyof typeof ChangeWatchErrorCode]
export const maxChangeWatchIdBytes = 57 as const
export const changeWatchIdPattern =
  "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}-[1-9][0-9]{0,19}$" as const
export const changeWatchLimits = {
  globalOwners: 64,
  principalOwners: 8,
  recordTargets: 1,
  catalogueTargets: 1,
} as const
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
/** Bounds the product schema puts on attachments and conversations, generated from it so no copy of a number can drift. */
export const bounds = {
  maxOrdinaryResponseBytes: 65536,
  maxRequestFrameBytes: 65536,
  maxReadyMethods: 52,
  maxAuthCredentialCharacters: 16384,
  maxProductClientIdCharacters: 256,
  maxProductSurfaceInstanceCharacters: 256,
  maxPhysicalRecordPayloadBytes: 65546,
  maxRecordPageRecords: 16,
  maxRecordPagePayloadBytes: 65546,
  maxRecordResponseBytes: 131072,
  minAgentInstallRequestIdCharacters: 1,
  maxAgentInstallRequestIdBytes: 256,
  maxConfiguredAgents: 3,
  maxAgentInstallVersionBytes: 128,
  maxImageBytes: 5242880,
  imageMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  maxMessageImages: 10,
  maxMessageImageBytes: 10485760,
  maxUploadBytes: 67108864,
  maxMessageFiles: 10,
  maxFilePathBytes: 4096,
  filePathPattern:
    "^(?:/(?!\\.{1,2}(?:/|$))[^/\\u0000-\\u001f\\u007f\\u0080-\\u009f]+)+$",
  conversationIdPattern: "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  maxConversationTitleBytes: 256,
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
export const ProductMethod = {
  SessionAuthenticate: "session.authenticate",
  AuthSession: "auth.session",
  ServerHealth: "server.health",
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
  ConversationAnswer: "conversation.answer",
  ConversationAnswerQuestion: "conversation.answerQuestion",
  ConversationCancel: "conversation.cancel",
  ConversationClose: "conversation.close",
  ConversationArchive: "conversation.archive",
  ConversationUnarchive: "conversation.unarchive",
  ConversationDelete: "conversation.delete",
  ConversationReorder: "conversation.reorder",
  AttachmentBegin: "attachment.begin",
  AgentsList: "agents.list",
  ConversationSetApprovalMode: "conversation.setApprovalMode",
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
  PairingCreate: "pairing.create",
  PairingPending: "pairing.pending",
  PairingStatus: "pairing.status",
  PairingApprove: "pairing.approve",
  PairingDeny: "pairing.deny",
  PairingCancel: "pairing.cancel",
  ConversationWatchRecords: "conversation.watchRecords",
  ConversationWatchCatalogue: "conversation.watchCatalogue",
  ConversationUnwatch: "conversation.unwatch",
} as const
export const ProductEvent = {
  SessionChallenge: "session.challenge",
  ConversationChanged: "conversation.changed",
  ConversationWatchEnded: "conversation.watchEnded",
} as const
export const ProductHandshakeMethod = "session.authenticate" as const
export const productReadyMethods = [
  "auth.session",
  "server.health",
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
  "conversation.answer",
  "conversation.answerQuestion",
  "conversation.cancel",
  "conversation.close",
  "conversation.archive",
  "conversation.unarchive",
  "conversation.delete",
  "conversation.reorder",
  "attachment.begin",
  "agents.list",
  "conversation.setApprovalMode",
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
  "pairing.create",
  "pairing.pending",
  "pairing.status",
  "pairing.approve",
  "pairing.deny",
  "pairing.cancel",
  "conversation.watchRecords",
  "conversation.watchCatalogue",
  "conversation.unwatch",
] as const
/** The grant each product method asks Cedar for, generated from protocol/product/manifest.json. null means another owner admits the method: the handshake, auth.session, or a watch. Writing to a conversation — an answer, an upload, an app's calls — asks for conversation.write; reading its records or catalogue asks for conversation.read; running a configured server, or enrolling a device, asks for credential.manage. */
export const productMethodGrants = {
  "session.authenticate": null,
  "auth.session": null,
  "server.health": "server.read",
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
  "conversation.answer": "conversation.write",
  "conversation.answerQuestion": "conversation.write",
  "conversation.cancel": "conversation.write",
  "conversation.close": "conversation.write",
  "conversation.archive": "conversation.write",
  "conversation.unarchive": "conversation.write",
  "conversation.delete": "conversation.write",
  "conversation.reorder": "conversation.write",
  "attachment.begin": "conversation.write",
  "agents.list": "server.read",
  "conversation.setApprovalMode": "conversation.write",
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
  "pairing.create": "credential.manage",
  "pairing.pending": "credential.manage",
  "pairing.status": "credential.manage",
  "pairing.approve": "credential.manage",
  "pairing.deny": "credential.manage",
  "pairing.cancel": "credential.manage",
  "conversation.watchRecords": null,
  "conversation.watchCatalogue": null,
  "conversation.unwatch": null,
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
