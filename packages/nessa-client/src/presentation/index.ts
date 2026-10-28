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
 *      ├── client.createThread() / sendMessage() / subscribe() / stopThread()
 *      ├── client.server.health()
 *      └── client.on("session.challenge", …)
 *
 * conversation-read-tools.ts adapts an injected conversation list/read API to
 * model tool descriptors and validated dispatch. It owns no socket or authority;
 * conversation-read-tools.test.ts checks that boundary with substituted APIs.
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
export type { GithubApi } from "./github-api.js"
export type { PairingApi } from "./pairing-api.js"
export type { ConversationApi } from "./conversation-api.js"
export {
  createConversationReadTools,
  type ConversationReadToolDescriptor,
  type ConversationReadTools,
  type ConversationReadToolsOptions,
} from "./conversation-read-tools.js"
export type {
  SendMessageOptions,
  StopThreadOptions,
  StopThreadResult,
  ThreadMessageMode,
  ThreadMessageReceipt,
} from "./thread-api.js"
export {
  NessaThreadSubscriptionError,
  type ThreadEventBatch,
  type ThreadSubscribeOptions,
  type ThreadSubscription,
} from "./thread-subscription.js"
export type { RecordReadApi } from "./record-read-api.js"
export type { AgentsApi } from "./agents-api.js"
export type { ServerApi } from "./server-api.js"
export type { WorkspaceApi } from "./workspace-api.js"
export type { ChannelApi } from "./channel-api.js"
export { channelRefusal } from "./channel-api.js"

export type { CatalogueReadApi } from "./catalogue-read-api.js"

export {
  createSubscriptionGate,
  type GatedSubscription,
  type SubscriptionGate,
} from "./subscription-gate.js"
export type {
  AgentsSubscriptionHandlers,
  ListSubscriptionHandlers,
  Subscription,
  SubscriptionApi,
  SubscriptionEnd,
  ViewFrame,
  ViewSubscriptionHandlers,
  EventSubscriptionHandlers,
} from "./subscription-api.js"

export type { VaultApi } from "./vault-api.js"
