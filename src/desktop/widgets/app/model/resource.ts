/**
 * An app's UI resource, read from a `resources/read` result (MCP Apps, *UI
 * Resource Format*): the content item for the URI the tool declared, of type
 * `text/html;profile=mcp-app`, its HTML from `text` or base64 `blob`, and the
 * CSP its `_meta.ui` declares. Anything else is not an app this host can load.
 */
import { appliedCsp, type AppliedCsp } from "./csp"
import { field, isObject, type Json, type JsonObject } from "./json-rpc"

/** The one type an app's resource may have. */
export const appMimeType = "text/html;profile=mcp-app"

export interface UiResource {
  readonly html: string
  readonly csp: AppliedCsp
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
  return { html, csp: appliedCsp(isObject(meta) ? field(meta, "ui") : undefined) }
}
