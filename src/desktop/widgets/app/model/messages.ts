/**
 * What an app's frame may say to the host, by method, read from an envelope
 * (`json-rpc.ts`) into the typed message the bridge acts on — or into the
 * error it answers with. The set of methods is closed: the requests and
 * notifications of MCP Apps (2026-01-26) a view sends, and the one the
 * sandbox proxy sends; anything else is answered as not found, or, for a
 * notification, ignored.
 *
 * Every field the bridge uses is read here from the envelope's copy, and the
 * typed message holds its own values: nothing downstream reads the copy
 * again by key.
 */
import {
  errorCodes,
  field,
  isObject,
  type Envelope,
  type ErrorCode,
  type Json,
  type JsonObject,
  type RequestId,
} from "./json-rpc"

/** A display mode, as MCP Apps names them. */
export type DisplayMode = "inline" | "fullscreen" | "pip"
const displayModes: readonly DisplayMode[] = ["inline", "fullscreen", "pip"]

/** The view's `ui/initialize`: what it calls itself and the modes it can be shown in. */
export interface Initialize {
  readonly protocolVersion: string
  readonly appName: string
  /** Absent when the view declared none: then it may be shown in any the host offers. */
  readonly displayModes?: readonly DisplayMode[]
}

/** A request from the view, by method. */
export type AppRequest =
  | { readonly method: "ui/initialize"; readonly initialize: Initialize }
  | { readonly method: "ping" }
  | {
      readonly method: "tools/call"
      readonly tool: string
      readonly arguments: JsonObject
    }
  | { readonly method: "resources/read"; readonly uri: string }
  | { readonly method: "ui/message"; readonly content: readonly JsonObject[] }
  | {
      readonly method: "ui/update-model-context"
      readonly content?: readonly JsonObject[]
      readonly structuredContent?: JsonObject
    }
  | { readonly method: "ui/open-link"; readonly url: string }
  | { readonly method: "ui/download-file"; readonly contents: readonly DownloadFile[] }
  | { readonly method: "ui/request-display-mode"; readonly mode: DisplayMode }

/** One file a view asks to download: an embedded resource's text or bytes. */
export interface DownloadFile {
  readonly uri: string
  readonly mimeType?: string
  readonly content:
    | { readonly kind: "text"; readonly text: string }
    | { readonly kind: "base64"; readonly blob: string }
}

/** A notification from the view, or from the sandbox proxy, by method. */
export type AppNotification =
  | { readonly method: "ui/notifications/initialized" }
  | {
      readonly method: "ui/notifications/size-changed"
      readonly width?: number
      readonly height?: number
    }
  | { readonly method: "ui/notifications/request-teardown" }
  | { readonly method: "notifications/message"; readonly level: string }
  | { readonly method: "ui/notifications/sandbox-proxy-ready" }
  | {
      readonly method: "ui/notifications/sandbox-csp-violation"
      /** The blocked origin, when the blocked thing had one. */
      readonly origin?: string
    }

/** One message from the frame, as the bridge acts on it. */
export type FromFrame =
  | { readonly kind: "request"; readonly id: RequestId; readonly request: AppRequest }
  | { readonly kind: "notification"; readonly notification: AppNotification }
  /** The view's answer to a request the host sent (`ui/resource-teardown`). */
  | { readonly kind: "answer"; readonly id: RequestId }
  /** A request the host answers with an error, unread past what made it one. */
  | {
      readonly kind: "refused"
      readonly id: RequestId
      readonly code: ErrorCode
      readonly message: string
    }
  /** Nothing the host answers or acts on. */
  | { readonly kind: "ignored" }

const nameLength = 512
const uriLength = 8 * 1024
const downloadCount = 16

function text(value: Json | undefined, limit = nameLength): string | undefined {
  return typeof value === "string" && value.length <= limit ? value : undefined
}

function size(value: Json | undefined): number | undefined | null {
  if (value === undefined) return undefined
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null
}

/** Content blocks: an array of objects that each name a `type`. */
function blocks(value: Json | undefined): readonly JsonObject[] | undefined {
  if (!Array.isArray(value)) return undefined
  const out: JsonObject[] = []
  for (const block of value as readonly Json[]) {
    if (!isObject(block) || typeof field(block, "type") !== "string") return undefined
    out.push(block)
  }
  return out
}

function readInitialize(params: JsonObject): Initialize | undefined {
  const protocolVersion = text(field(params, "protocolVersion"))
  const appInfo = field(params, "appInfo")
  const capabilities = field(params, "appCapabilities")
  if (protocolVersion === undefined || !isObject(appInfo) || !isObject(capabilities))
    return undefined
  const appName = text(field(appInfo, "name"))
  if (appName === undefined) return undefined
  const declared = field(capabilities, "availableDisplayModes")
  if (declared === undefined) return { protocolVersion, appName }
  if (!Array.isArray(declared)) return undefined
  const modes: DisplayMode[] = []
  for (const mode of declared as readonly Json[]) {
    const known = displayModes.find((each) => each === mode)
    if (known === undefined) return undefined
    if (!modes.includes(known)) modes.push(known)
  }
  return { protocolVersion, appName, displayModes: modes }
}

function readDownloads(value: Json | undefined): readonly DownloadFile[] | undefined {
  if (!Array.isArray(value) || value.length === 0 || value.length > downloadCount)
    return undefined
  const out: DownloadFile[] = []
  for (const item of value as readonly Json[]) {
    // Only an embedded resource: a link would have the host fetch for the view.
    if (!isObject(item) || field(item, "type") !== "resource") return undefined
    const resource = field(item, "resource")
    if (!isObject(resource)) return undefined
    const uri = text(field(resource, "uri"), uriLength)
    const mimeType = field(resource, "mimeType")
    if (uri === undefined || (mimeType !== undefined && text(mimeType) === undefined))
      return undefined
    const body = field(resource, "text")
    const blob = field(resource, "blob")
    const content =
      typeof body === "string" && blob === undefined
        ? ({ kind: "text", text: body } as const)
        : typeof blob === "string" && body === undefined
          ? ({ kind: "base64", blob } as const)
          : undefined
    if (content === undefined) return undefined
    out.push({ uri, ...(typeof mimeType === "string" ? { mimeType } : {}), content })
  }
  return out
}

const invalid = (id: RequestId): FromFrame => ({
  kind: "refused",
  id,
  code: errorCodes.invalidParams,
  message: "Invalid params",
})

function readRequest(id: RequestId, method: string, params: JsonObject): FromFrame {
  const ok = (request: AppRequest): FromFrame => ({ kind: "request", id, request })
  switch (method) {
    case "ui/initialize": {
      const initialize = readInitialize(params)
      return initialize ? ok({ method, initialize }) : invalid(id)
    }
    case "ping":
      return ok({ method })
    case "tools/call": {
      const tool = text(field(params, "name"))
      const args = field(params, "arguments")
      if (
        tool === undefined ||
        tool.length === 0 ||
        (args !== undefined && !isObject(args))
      )
        return invalid(id)
      return ok({ method, tool, arguments: args ?? {} })
    }
    case "resources/read": {
      const uri = text(field(params, "uri"), uriLength)
      return uri === undefined || uri.length === 0 ? invalid(id) : ok({ method, uri })
    }
    case "ui/message": {
      const content = blocks(field(params, "content"))
      return field(params, "role") === "user" && content && content.length > 0
        ? ok({ method, content })
        : invalid(id)
    }
    case "ui/update-model-context": {
      const rawContent = field(params, "content")
      const rawStructured = field(params, "structuredContent")
      const content = rawContent === undefined ? undefined : blocks(rawContent)
      if (rawContent !== undefined && content === undefined) return invalid(id)
      if (rawStructured !== undefined && !isObject(rawStructured)) return invalid(id)
      return ok({
        method,
        ...(content ? { content } : {}),
        ...(isObject(rawStructured) ? { structuredContent: rawStructured } : {}),
      })
    }
    case "ui/open-link": {
      const url = text(field(params, "url"), uriLength)
      return url === undefined ? invalid(id) : ok({ method, url })
    }
    case "ui/download-file": {
      const contents = readDownloads(field(params, "contents"))
      return contents ? ok({ method, contents }) : invalid(id)
    }
    case "ui/request-display-mode": {
      const mode = displayModes.find((each) => each === field(params, "mode"))
      return mode === undefined ? invalid(id) : ok({ method, mode })
    }
    default:
      return {
        kind: "refused",
        id,
        code: errorCodes.methodNotFound,
        message: "Method not found",
      }
  }
}

function readNotification(method: string, params: JsonObject): FromFrame {
  const ok = (notification: AppNotification): FromFrame => ({
    kind: "notification",
    notification,
  })
  switch (method) {
    case "ui/notifications/initialized":
    case "ui/notifications/request-teardown":
    case "ui/notifications/sandbox-proxy-ready":
      return ok({ method })
    case "ui/notifications/size-changed": {
      const width = size(field(params, "width"))
      const height = size(field(params, "height"))
      if (width === null || height === null) return { kind: "ignored" }
      return ok({
        method,
        ...(width === undefined ? {} : { width }),
        ...(height === undefined ? {} : { height }),
      })
    }
    case "notifications/message": {
      const level = text(field(params, "level"), 32)
      return level === undefined ? { kind: "ignored" } : ok({ method, level })
    }
    case "ui/notifications/sandbox-csp-violation": {
      const origin = text(field(params, "origin"))
      return ok(origin === undefined ? { method } : { method, origin })
    }
    default:
      return { kind: "ignored" }
  }
}

/** What the bridge acts on for one envelope. */
export function readFromFrame(envelope: Envelope): FromFrame {
  switch (envelope.kind) {
    case "request":
      return readRequest(envelope.id, envelope.method, envelope.params)
    case "notification":
      return readNotification(envelope.method, envelope.params)
    case "result":
    case "error":
      return { kind: "answer", id: envelope.id }
    case "malformed":
      return envelope.id === null
        ? { kind: "ignored" }
        : {
            kind: "refused",
            id: envelope.id,
            code: errorCodes.invalidRequest,
            message: "Invalid request",
          }
  }
}
