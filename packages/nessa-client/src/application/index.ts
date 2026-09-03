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
 * runConnectHandshake(session, options) ──► HelloOk
 * ```
 */
export { runConnectHandshake, waitForConnectChallenge } from "./connect-flow.js"
export type { EventHandler, NessaClientEvents } from "./events.js"
export type { NessaClientConnectOptions } from "./options.js"
export { NessaRpcError } from "./rpc-error.js"
