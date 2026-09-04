/**
 * Wire types generated from `protocol/schemas/v1/`.
 * Regenerate: `pnpm protocol:generate`
 */
export type {
  AuthToken,
  ClientInfo,
  ClientRole,
  ConnectChallenge,
  ConnectParams,
  EchoParams,
  EchoResult,
  EventFrame,
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
} from "../generated/protocol.js"

import type { EventFrame, ReqFrame, ResFrame } from "../generated/protocol.js"

/** Any JSON message on the WebSocket. */
export type Frame = ReqFrame | ResFrame | EventFrame

import type { ConnectChallenge } from "../generated/protocol.js"
import { Event } from "../generated/catalog.js"

/** Server push events the client understands today. */
export type ClientEventMap = {
  [Event.ConnectChallenge]: ConnectChallenge
}
