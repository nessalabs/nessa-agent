/**
 * An app's UI resource, read from a `resources/read` result (MCP Apps, *UI
 * Resource Format*): the content item for the URI the tool declared, of type
 * `text/html;profile=mcp-app`, its HTML from `text` or base64 `blob`, the
 * CSP its `_meta.ui` declares, and the device permissions it asks for. Anything
 * else is not an app this host can load.
 */
import { appliedCsp, type AppliedCsp } from "./csp"
import { field, isObject, type Json, type JsonObject } from "./json-rpc"

/** The one type an app's resource may have. */
export const appMimeType = "text/html;profile=mcp-app"

/**
 * The device permissions this host grants an app that asks (`_meta.ui.permissions`,
 * a flag or an object per permission). Only the microphone: a voice app needs it,
 * and nothing asks for the others yet.
 */
export interface AppPermissions {
  readonly microphone: boolean
}

export interface UiResource {
  readonly html: string
  readonly csp: AppliedCsp
  readonly permissions: AppPermissions
}

function asked(permissions: Json | undefined, name: string): boolean {
  if (!isObject(permissions)) return false
  const value = field(permissions, name)
  return value === true || isObject(value)
}

/**
 * The app frame's `allow` attribute for what was granted: the one place the
 * permissions policy is written, for the proxy to set on the app's frame
 * (`sandbox/proxy.html` accepts only this spelling or nothing).
 */
export function frameAllow(permissions: AppPermissions): string {
  return permissions.microphone ? "microphone *" : ""
}

function decodeBase64(blob: string): string | undefined {
  try {
    const binary = atob(blob)
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    return undefined
  }
}

/** The app in `result` for `uri`, or `undefined` when it holds none this host can load. */
export function uiResource(result: JsonObject, uri: string): UiResource | undefined {
  const contents = field(result, "contents")
  if (!Array.isArray(contents)) return undefined
  const item = (contents as readonly Json[]).find(
    (entry): entry is JsonObject => isObject(entry) && field(entry, "uri") === uri,
  )
  if (!item || field(item, "mimeType") !== appMimeType) return undefined
  const text = field(item, "text")
  const blob = field(item, "blob")
  const html =
    typeof text === "string" && blob === undefined
      ? text
      : typeof blob === "string" && text === undefined
        ? decodeBase64(blob)
        : undefined
  if (html === undefined || html.trim() === "") return undefined
  const meta = field(item, "_meta")
  const ui = isObject(meta) ? field(meta, "ui") : undefined
  return {
    html,
    csp: appliedCsp(ui),
    permissions: { microphone: asked(isObject(ui) ? field(ui, "permissions") : undefined, "microphone") },
  }
}
