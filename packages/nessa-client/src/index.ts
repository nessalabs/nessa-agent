/**
 * @nessa/client — typed WebSocket SDK for the Nessa Server Client API.
 *
 * ```text
 *                    connectClient (composition)
 *                           │
 *           ┌───────────────┼───────────────┐
 *           │               │               │
 *    presentation      application     transport
 *    (NessaClient)     (handshake)     (WireSession)
 *           │               │               │
 *           └───────────────┴───────────────┘
 *                           │
 *                      protocol (typed frames)
 * ```
 */
export {
  NessaClient,
  type AuthApi,
  type CredentialApi,
  type CredentialGrant,
  type CredentialMetadata,
  type IssueCredentialParams,
  type IssueCredentialResult,
  type ServerApi,
} from "./presentation/index.js"
export type {
  NessaClientConnectOptions,
  ProductConnectOptions,
} from "./application/index.js"
export type { NessaClientEvents } from "./application/index.js"
export {
  isLoopbackWebSocketUrl,
  isStage,
  NessaRpcError,
  NessaProtocolCompatibilityError,
  NessaConnectionClosedError,
  resolveConnectOptions,
  stageAllowsDefaultUrl,
  StageConfigError,
  STAGES,
  Stage,
} from "./application/index.js"
export type {
  ClientInfo,
  EchoParams,
  EchoResult,
  EventFrame,
  Frame,
  GatewayError,
  HealthResult,
  ReqFrame,
  ResFrame,
  ShortcutArgs,
  ShortcutBinding,
  ShortcutsDocument,
  SurfaceInfo,
  ProductSessionReady,
  SessionAuthenticateParams,
  SessionChallenge,
} from "./protocol/index.js"
export type { EventName, MethodName } from "./protocol/index.js"
export { Event, Method } from "./protocol/index.js"
export { ProductEvent, ProductMethod } from "./protocol/index.js"

export type { ProductConnectRetryOptions } from "./application/connect-retry.js"

export { NessaClientConfig, type NessaClientConfigOptions } from "./application/index.js"

export {
  PrincipalKind,
  MembershipRole,
  MembershipState,
  SessionCloseReason,
} from "./generated/product.js"
export {
  NessaSessionUnavailableError,
  type ConnectionState,
} from "./application/managed-session.js"

export {
  ClientRole,
  ClientPlatform,
  SurfaceKind,
  Scope,
  ShortcutAction,
  ShortcutScope,
  ShortcutSurface,
} from "./generated/protocol.js"
export const ConnectionProfile = { Product: "product" } as const

export type { CommonConnectOptions } from "./application/options.js"
export type { EventHandler } from "./application/events.js"
export type { ClientEventMap } from "./protocol/types.js"
export type { ConversationApi } from "./presentation/conversation-api.js"
export type { ResolvedConnectRetryOptions } from "./application/connect-retry.js"
export type {
  ResolvedConnectOptions,
  ResolvedProductConnectOptions,
} from "./application/resolve-options.js"
export type {
  ProductGrant,
  ProductCredentialMetadata,
  CredentialIssueParams,
  IssuedCredentialResult,
  ExistingCredentialResult,
  ProductClientMetadata,
  CredentialListResult,
  CredentialRevokeResult,
} from "./generated/product.js"

export type { SessionTermination } from "./generated/product.js"

export { NessaMutationError } from "./application/mutation-error.js"
export type {
  ProductMembership,
  ProductPrincipal,
  ProductResource,
} from "./generated/product.js"

export {
  NessaCredentialUnavailableError,
  type CredentialSource,
} from "./application/credential-source.js"
export { LocalFileCredentialSource } from "./transport/local-credential-source.js"
