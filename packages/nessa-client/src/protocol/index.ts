/**
 * Protocol layer — typed wire frames aligned with `protocol/schemas/v1/`.
 *
 * Decode incoming JSON once at the boundary; handlers work with typed frames,
 * not untyped `JSON.parse` results.
 *
 * ```
 * WebSocket text
 *      │
 *      ▼
 * parseWireMessage ──► Frame
 *      │
 *      ├── event  → application event bus
 *      └── res    → transport request registry
 * ```
 */
export type {
  AuthToken,
  ClientEventMap,
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
} from "./types.js"

export { Event, Method, type EventName, type MethodName } from "../generated/catalog.js"
export { isEventFrame, isResponseFrame, parseWireMessage } from "./decode.js"
export { assertConnectChallenge, assertEchoResult, assertHealthResult, assertHelloOk, assertPingResult } from "./validate.js"
export { buildRequestFrame, encodeWireMessage } from "./encode.js"
