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
export type { AttachmentApi } from "./attachment-api.js"
export type { McpAppsApi } from "./mcp-apps-api.js"
export type { McpServersApi } from "./mcp-servers-api.js"
export type { ConversationApi } from "./conversation-api.js"
export type { RecordReadApi } from "./record-read-api.js"
export type { AgentsApi } from "./agents-api.js"
export type { ServerApi } from "./server-api.js"

export type { CatalogueReadApi } from "./catalogue-read-api.js"

export type { ChangeWatchApi } from "./change-watch-api.js"
