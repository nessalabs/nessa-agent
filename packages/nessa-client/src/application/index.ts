/**
 * Application layer — connect flow and session policy.
 *
 * Orchestrates the handshake sequence (challenge → connect) on top of
 * transport. No React, no UI — just use-case steps the server expects.
 *
 * ```
 * waitForConnectChallenge(session)
 *      │
 *      ▼
 * runProductHandshake(session, options) ──► ProductSessionReady
 * ```
 */
export type { EventHandler, NessaClientEvents } from "./events.js"
export type { NessaClientConnectOptions, ProductConnectOptions } from "./options.js"
export {
  isLoopbackWebSocketUrl,
  resolveConnectOptions,
  type ResolvedProductConnectOptions,
  type ResolvedConnectOptions,
} from "./resolve-options.js"
export { NessaRpcError } from "./rpc-error.js"
export { NessaConnectionClosedError } from "./connection-closed-error.js"
export {
  isStage,
  stageAllowsDefaultUrl,
  StageConfigError,
  STAGES,
  Stage,
} from "./stage.js"

export { NessaProtocolCompatibilityError } from "./protocol-compatibility-error.js"

export type { ProductConnectRetryOptions } from "./connect-retry.js"

export { NessaClientConfig, type NessaClientConfigOptions } from "./client-config.js"
