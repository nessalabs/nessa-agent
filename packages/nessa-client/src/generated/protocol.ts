/* eslint-disable */
/** Low-level RPC envelope. SDK API methods create these frames and correlate responses by id. */

export interface ReqFrame {
  /** Request frame discriminator. */

  type: "req"
  /** Correlation identifier echoed by the response. */

  id: string
  /** Registered RPC method name. */

  method: string
  /** Method-specific request payload. */

  params: unknown
}

/** RPC response envelope matched to a pending request. Failed responses become NessaRpcError. */

export interface ResFrame {
  /** Response frame discriminator. */

  type: "res"
  /** Identifier of the request being answered. */

  id: string
  /** Whether the operation succeeded. */

  ok: boolean
  /** Method-specific result on success. */

  payload?: unknown
  /** Structured rejection on failure. */
  error?: GatewayError
}

/** Structured server rejection carried by a response frame and exposed as NessaRpcError. */

export interface GatewayError {
  /** Machine-readable error identifier. */

  code: string
  /** Server-provided explanation of the rejection. */

  message: string
  /** Optional structured context; its shape depends on the error code. */

  details?: unknown
}

/** Unsolicited protocol event delivered to matching client.on subscribers. */

export interface EventFrame {
  /** Event frame discriminator. */

  type: "event"
  /** Protocol event name. */

  event: string
  /** Event-specific data. */

  payload: unknown
  /** Server event sequence number. */

  seq: number
  /** Server state version associated with the event. */

  stateVersion: number
}

/** Gateway health response from client.server.health(). On product connections, access is authorized by the server. */

export interface HealthResult {
  /** Whether the health probe succeeded. */

  ok: boolean
  /** Current runtime availability reported by the gateway. */

  runtimeStatus: "ready" | "starting" | "unavailable" | "error"
  /** Time since server startup in milliseconds. */

  uptimeMs: number
}

/** Text input for the authorized conversation echo round trip. */

export interface EchoParams {
  /** User draft text to echo back. */

  text: string
}

/** Echoed input text; no model-generated reply is produced. */

export interface EchoResult {
  /** Echo of EchoParams.text. */

  text: string
}

/** Versioned server-owned shortcut configuration included in the legacy handshake. */

export interface ShortcutsDocument {
  /** Shortcut document format version. */

  version: 1
  /** Ordered keyboard bindings supplied by the server. */

  bindings: ShortcutBinding[]
}

/** Maps a keyboard accelerator to a product action for a given scope and surface. */

export interface ShortcutBinding {
  /** Tauri-style accelerator, e.g. CmdOrCtrl+Shift+D. */

  keys: string
  /** Action requested when the binding fires. */

  action: "panel.summon" | "panel.newTab" | "panel.closeTab" | "panel.activateTab"
  /** Optional action-specific arguments. */
  args?: ShortcutArgs
  /** Context in which the binding applies. */

  scope: "global" | "focused"
  /** Surface on which the binding applies. */

  surface: "desktop" | "browser" | "*"
}

/** Optional arguments attached to a server-owned shortcut action. */

export interface ShortcutArgs {
  /** Zero-based index among open tabs. */

  index?: number
  /** Preferred later: pin a binding to a conversation id when open. */

  conversationId?: string
}

/** Action identifiers supported by server-owned keyboard bindings. */

export type ShortcutAction =
  "panel.summon" | "panel.newTab" | "panel.closeTab" | "panel.activateTab"

/** Contexts in which a shortcut binding applies. */

export type ShortcutScope = "global" | "focused"

/** Surface selectors for server-owned keyboard bindings. */

export type ShortcutSurface = "desktop" | "browser" | "*"

/** Caller role metadata; authorization comes from the credential and membership. */

export type ClientRole = "surface"

/** Shared server-read scope name. Product grants carry exact actions and resources. */

export type Scope = "server.read"

/** Describes the connecting UI or CLI; verified identity comes from its credential. */

export interface SurfaceInfo {
  /** Kind of user-facing surface. */

  kind: "panel" | "web" | "desktop" | "cli"
  /** Identifier distinguishing instances of this surface. */

  instance: string
}

/** Application metadata supplied when connecting. The product handshake sends only id. */

export interface ClientInfo {
  /** Client application or instance identifier. */

  id: string
  /** Client software version. */

  version: string
  /** Runtime or host platform. Use other when none applies. */

  platform: "node" | "browser" | "macos" | "linux" | "windows" | "other"
}

/** Kinds of surfaces that can describe a connection. */

export type SurfaceKind = "panel" | "web" | "desktop" | "cli"

/** Runtime and host platform identifiers used in client metadata. */

export type ClientPlatform = "node" | "browser" | "macos" | "linux" | "windows" | "other"

export const ClientRole = { Surface: "surface" } as const
export const Scope = { ServerRead: "server.read" } as const
export const SurfaceKind = {
  Panel: "panel",
  Web: "web",
  Desktop: "desktop",
  Cli: "cli",
} as const
export const RuntimeStatus = {
  Ready: "ready",
  Starting: "starting",
  Unavailable: "unavailable",
  Error: "error",
} as const
export const ClientPlatform = {
  Node: "node",
  Browser: "browser",
  Macos: "macos",
  Linux: "linux",
  Windows: "windows",
  Other: "other",
} as const
export const ShortcutAction = {
  PanelSummon: "panel.summon",
  PanelNewTab: "panel.newTab",
  PanelCloseTab: "panel.closeTab",
  PanelActivateTab: "panel.activateTab",
} as const
export const ShortcutScope = { Global: "global", Focused: "focused" } as const
export const ShortcutSurface = {
  Desktop: "desktop",
  Browser: "browser",
  Any: "*",
} as const
