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
  EchoParams,
  EchoResult,
  EventFrame,
  Frame,
  GatewayError,
  HealthResult,
  HelloOk,
  PingParams,
  PingResult,
  ReqFrame,
  ResFrame,
  Scope,
  ShortcutAction,
  ShortcutArgs,
  ShortcutBinding,
  ShortcutScope,
  ShortcutSurface,
  ShortcutsDocument,
  SurfaceInfo,
} from "./protocol/index.js"
export type { EventName, MethodName } from "./protocol/index.js"
export { Event, Method } from "./protocol/index.js"
