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
export { NessaClient, type ServerApi } from "./presentation/index.js"
export type { NessaClientConnectOptions } from "./application/index.js"
export type { NessaClientEvents } from "./application/index.js"
export {
  DEV_AUTH_TOKEN,
  isLoopbackWebSocketUrl,
  isStage,
  NessaRpcError,
  resolveConnectOptions,
  stageAllowsDefaultAuth,
  StageConfigError,
  STAGES,
  type Stage,
} from "./application/index.js"
export type {
  AuthToken,
  ClientInfo,
  ClientRole,
  ConnectChallenge,
  ConnectParams,
  EventFrame,
  Frame,
  GatewayError,
  HealthResult,
  HelloOk,
  ReqFrame,
  ResFrame,
  Scope,
  SurfaceInfo,
} from "./protocol/index.js"
export type { EventName, MethodName } from "./protocol/index.js"
export { Event, Method } from "./protocol/index.js"
