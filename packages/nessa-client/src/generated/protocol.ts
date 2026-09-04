/* eslint-disable */
/**
 * Generated from protocol/schemas/v1/*.json — do not edit by hand.
 *
 * **Source of truth:** `protocol/schemas/v1/` (payload shapes).
 * Method/event *names* → see `./catalog.ts` (from `protocol/manifest.json`).
 * Regenerate: `pnpm protocol:generate`
 */

export interface ReqFrame {
  type: "req"
  id: string
  method: string
  params: unknown
}

export interface ResFrame {
  type: "res"
  id: string
  ok: boolean
  payload?: unknown
  error?: GatewayError
}

export interface GatewayError {
  code: string
  message: string
  details?: unknown
}

export interface EventFrame {
  type: "event"
  event: string
  payload: unknown
  seq: number
  stateVersion: number
}

export interface ConnectParams {
  minProtocol: number
  maxProtocol: number
  role: "surface"
  surface: SurfaceInfo
  client: ClientInfo
  auth: AuthToken
}

export interface SurfaceInfo {
  kind: "panel" | "web" | "desktop" | "cli"
  instance: string
}

export interface ClientInfo {
  id: string
  version: string
  platform: string
}

export interface AuthToken {
  token: string
  /**
   * Echo of connect.challenge nonce — binds this connect RPC to the open socket.
   */
  nonce: string
}

export interface HelloOk {
  protocol: 1
  scopes: "server.read"[]
  serverVersion: string
  runtimeStatus: "ready" | "starting" | "unavailable" | "error"
  policy: ServerPolicy
  shortcuts: ShortcutsDocument
}

export interface ServerPolicy {
  maxPayloadBytes: number
}

export interface ShortcutsDocument {
  version: 1
  bindings: ShortcutBinding[]
}

export interface ShortcutBinding {
  /**
   * Tauri-style accelerator, e.g. CmdOrCtrl+Shift+D.
   */
  keys: string
  action: "panel.summon" | "panel.newTab" | "panel.closeTab" | "panel.activateTab"
  args?: ShortcutArgs
  scope: "global" | "focused"
  surface: "desktop" | "browser" | "*"
}

export interface ShortcutArgs {
  /**
   * Zero-based index among open tabs.
   */
  index?: number
  /**
   * Preferred later: pin a binding to a conversation id when open.
   */
  conversationId?: string
}

export interface ConnectChallenge {
  nonce: string
  protocol: 1
}

export interface HealthResult {
  ok: boolean
  runtimeStatus: "ready" | "starting" | "unavailable" | "error"
  uptimeMs: number
}

export type ShortcutAction =
  "panel.summon" | "panel.newTab" | "panel.closeTab" | "panel.activateTab"

export type ShortcutScope = "global" | "focused"

export type ShortcutSurface = "desktop" | "browser" | "*"

export type ClientRole = "surface"

export type Scope = "server.read"
