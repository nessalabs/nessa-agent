/**
 * Presentation layer — stable public API for product code.
 *
 * Surfaces and plugins import `NessaClient` from here (via package root).
 * No wire types or WebSocket details leak into UI code.
 *
 * ```
 * NessaClient.connect(options)
 *      │
 *      ├── client.productSession (ProductSessionReady)
 *      ├── client.server.health()
 *      └── client.on("session.challenge", …)
 * ```
 */
export { NessaClient } from "./nessa-client.js"
export type { AuthApi } from "./auth-api.js"
export type {
  CredentialApi,
  CredentialGrant,
  CredentialMetadata,
  IssueCredentialParams,
  IssueCredentialResult,
  PrincipalKind,
} from "./credential-api.js"
export type { NessaClientConnectOptions } from "../application/options.js"
export type { ConversationApi } from "./conversation-api.js"
export type { ServerApi } from "./server-api.js"
