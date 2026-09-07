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
export const ProductMethod = {
  SessionAuthenticate: "session.authenticate",
  AuthSession: "auth.session",
  ServerHealth: "server.health",
  CredentialIssue: "credential.issue",
  CredentialList: "credential.list",
  CredentialRevoke: "credential.revoke",
  ConversationEcho: "conversation.echo",
} as const
export const ProductEvent = { SessionChallenge: "session.challenge" } as const
