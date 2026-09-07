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
  ClientEventMap,
  ClientInfo,
  ClientRole,
  EchoParams,
  EchoResult,
  EventFrame,
  Frame,
  GatewayError,
  HealthResult,
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
  ProductSessionReady,
  SessionAuthenticateParams,
  SessionChallenge,
} from "./types.js"

export { Event, Method, type EventName, type MethodName } from "../generated/catalog.js"
export { isEventFrame, isResponseFrame, parseWireMessage } from "./decode.js"
export { assertEchoResult, assertHealthResult } from "./validate.js"
export { assertProductSessionReady, assertSessionChallenge } from "./validate.js"
export { ProductEvent, ProductMethod } from "./product-types.js"
export { buildRequestFrame, encodeWireMessage } from "./encode.js"
