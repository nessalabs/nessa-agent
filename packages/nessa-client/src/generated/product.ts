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
  /** Current invocation state. */
  status: ConversationMessageStatus
  /** Bounded diagnostic for this invocation. */
  error?: string
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
  /** Queue or steering admission. */
  mode: ConversationPendingMode
}
/** An exact option offered by the provider. */
export interface ConversationPermissionOption {
  /** Opaque offered option identifier. */
  id: string
  /** Provider label for this option. */
  label: string
}
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
  /** Current provider tool status. */
  status: string
  /** Bounded provider-observed tool output and file changes; omitted content is marked. */
  details: string
  /** Exact tool arguments observed through a permission request, or empty when unavailable. */
  input: string
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
  /** Some non-actionable history or text was omitted to bound this response. */
  truncated: boolean
  /** Why pending review choices cannot safely be shown; do not offer inferred choices. */
  permissionViewError?: string
  /** All currently pending execution IDs are represented, so an exact reorder may be attempted. */
  queueComplete: boolean
  /** Configured provider, model and working directory for this conversation. */
  runtime?: ConversationRuntime
}
/** Idempotently create or reopen one named conversation. */
export interface ConversationCreateParams {
  /** Canonical lowercase hyphenated UUID identifying the conversation within the authenticated organization. */
  conversationId: string
  /** Stable action identifier retained for retries of one logical command. */
  requestId: string
  /** Coding agent this conversation runs on for its whole life. Omitted takes the gateway's configured default. Ignored when the conversation already exists, which is reopened on the agent it was created with. */
  agent?: string
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
/** Runtime configuration selected by the gateway composition for this conversation. */
export interface ConversationRuntime {
  /** Configured model identifier. */
  model: string
  /** Configured agent provider name. */
  provider: string
  /** Working directory on the gateway host, not the client filesystem. */
  workspace: string
}
/** One ordered observation fragment in a bounded execution projection; offsets may have gaps. */
export interface ConversationPart {
  /** Zero-based SDK observation offset within the owning execution, used to preserve order. */
  offset: number
  /** Text, exposed thought content, or a tool observation. */
  kind: "text" | "thought" | "tool"
  /** Exact text fragment for text or thought observations; empty for tool observations. */
  text: string
  /** Owning tool identity for a tool observation; empty for text or thought observations. */
  toolId: string
  /** Opaque provider message identity; only fragments with the same identity may be combined. */
  messageId?: string
}
/** Typed rejection code carried by a conversation command the gateway dispatched and refused. Branch on these instead of message text. These are not every code a conversation request can receive: access and routing failures are answered by the session before a conversation command is dispatched, and carry their own codes. agent_startup_deadline means the agent was still starting when its budget expired, so nothing reached the provider and the same command is safe to repeat; it normally succeeds once the runtime is warm, but a launch whose process could not be confirmed stopped keeps that conversation blocked. invalid_request and agent_not_configured reject the command until their cause is addressed. agent_not_configured, agent_unsupported and conversations_not_configured are three different situations and only one of them is fixed by configuring an agent: the gateway runs no conversations at all, it names no runtime under the agent this conversation asked for, or no build here can open that conversation's agent. The image codes answer `attachment.begin` and a message naming uploads: image_input_unsupported is a model that takes no images, so no ticket and no message with one will ever be taken; attachment_not_found is an image this conversation does not hold — never uploaded into it, expired, or released when it closed; attachment_unavailable is one it holds but could not read; attachment_capacity is no room for another upload right now; attachment_storage_unavailable is the gateway unable to keep the bytes. attachment_cleanup_unavailable is a close that did happen, whose release of this conversation's uploads did not, and is the one image code that is not a refusal of the command. */
export const ConversationErrorCode = {
  AgentNotConfigured: "agent_not_configured",
  AgentUnsupported: "agent_unsupported",
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
} as const
export type ConversationErrorCode =
  (typeof ConversationErrorCode)[keyof typeof ConversationErrorCode]
/** Bounds the product schema puts on attachments, generated from it so no copy of a number can drift. */
export const bounds = {
  maxImageBytes: 5242880,
  imageMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  maxMessageImages: 10,
  maxMessageImageBytes: 10485760,
  maxUploadBytes: 67108864,
  maxMessageFiles: 10,
  maxFilePathBytes: 4096,
  filePathPattern:
    "^(?:/(?!\\.{1,2}(?:/|$))[^/\\u0000-\\u001f\\u007f\\u0080-\\u009f]+)+$",
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
  ConversationSend: "conversation.send",
  ConversationSteer: "conversation.steer",
  ConversationRemove: "conversation.remove",
  ConversationAnswer: "conversation.answer",
  ConversationCancel: "conversation.cancel",
  ConversationClose: "conversation.close",
  ConversationReorder: "conversation.reorder",
  AttachmentBegin: "attachment.begin",
} as const
export const ProductEvent = { SessionChallenge: "session.challenge" } as const
