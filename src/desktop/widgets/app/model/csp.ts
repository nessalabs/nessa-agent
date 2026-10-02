/**
 * The Content Security Policy an app's document is loaded under, built only
 * from its resource's `_meta.ui.csp` (MCP Apps, *UI Resource Format*, *Host
 * Behavior*): with nothing declared, the app has no network at all.
 *
 * Construction, not enumeration (gate 11): each declared domain is parsed
 * into a scheme, a host and a port, and the policy is written from those
 * parts. A keyword (`'unsafe-eval'`), a bare `*`, a scheme alone (`https:`),
 * a path, a `;` or whitespace cannot be spelled by the parts, so cannot reach
 * the policy. A domain that does not parse is left out — a further
 * restriction, which the spec allows — and what was applied is what the host
 * tells the app it approved (`hostCapabilities.sandbox.csp`).
 *
 * The one owner of the policy: the host writes it into the app's document
 * (`appDocument`), and the sandbox proxy only loads that document.
 */
import { field, isObject, type Json, type JsonObject } from "./json-rpc"

/** A source the policy may name: a scheme, a host (perhaps `*.`-prefixed), a port. */
export interface CspSource {
  readonly scheme: "https" | "http" | "wss" | "ws"
  readonly host: string
  readonly port?: number
}

/** The domains a resource declared that the host applies, by what they are for. */
export interface AppliedCsp {
  /** `connect-src`: fetch, XHR, WebSocket. */
  readonly connect: readonly CspSource[]
  /** `script-src`, `style-src`, `img-src`, `font-src`, `media-src`. */
  readonly resource: readonly CspSource[]
  /** `frame-src`: nested frames. */
  readonly frame: readonly CspSource[]
  /** `base-uri`. */
  readonly baseUri: readonly CspSource[]
}

/** At most this many domains per list are applied; the rest are left out. */
export const domainsPerList = 32
/** A domain longer than this is left out. */
export const domainLength = 253

const schemes: readonly CspSource["scheme"][] = ["https", "http", "wss", "ws"]
const label = "[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
const ipv4 =
  "(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)(?:\\.(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)){3}"
const domainPattern = new RegExp(
  `^(https|http|wss|ws)://((?:\\*\\.)?${label}(?:\\.${label})*|${ipv4})(?::(\\d{1,5}))?/?$`,
)

/** Parses one declared domain into its parts, or `undefined` when it is not one. */
export function parseSource(declared: Json | undefined): CspSource | undefined {
  if (typeof declared !== "string" || declared.length > domainLength) return undefined
  const match = domainPattern.exec(declared.toLowerCase())
  const scheme = schemes.find((each) => each === match?.[1])
  const host = match?.[2]
  if (scheme === undefined || host === undefined) return undefined
  const port = match?.[3]
  if (port === undefined) return { scheme, host }
  const number = Number(port)
  if (number < 1 || number > 65535) return undefined
  return { scheme, host, port: number }
}

/** A source as the policy spells it, from its parts. */
export function sourceText(source: CspSource): string {
  return `${source.scheme}://${source.host}${source.port === undefined ? "" : `:${source.port}`}`
}

function list(csp: JsonObject | undefined, key: string): readonly CspSource[] {
  const declared = csp ? field(csp, key) : undefined
  if (!Array.isArray(declared)) return []
  const out: CspSource[] = []
  const seen = new Set<string>()
  for (const entry of declared as readonly Json[]) {
    if (out.length === domainsPerList) break
    const source = parseSource(entry)
    if (!source) continue
    const spelled = sourceText(source)
    if (seen.has(spelled)) continue
    seen.add(spelled)
    out.push(source)
  }
  return out
}

/** What the host applies of a resource's `_meta.ui.csp` (its `ui` object, or nothing). */
export function appliedCsp(ui: Json | undefined): AppliedCsp {
  const csp = isObject(ui) ? field(ui, "csp") : undefined
  const declared = isObject(csp) ? csp : undefined
  return {
    connect: list(declared, "connectDomains"),
    resource: list(declared, "resourceDomains"),
    frame: list(declared, "frameDomains"),
    baseUri: list(declared, "baseUriDomains"),
  }
}

const spell = (sources: readonly CspSource[]) => sources.map(sourceText)

/** The policy, one directive per entry, in the spec's order. */
export function cspPolicy(csp: AppliedCsp): string {
  const resource = spell(csp.resource)
  const or = (sources: readonly CspSource[], otherwise: string) =>
    sources.length === 0 ? [otherwise] : spell(sources)
  const directives: [string, readonly string[]][] = [
    ["default-src", ["'none'"]],
    ["script-src", ["'self'", "'unsafe-inline'", ...resource]],
    ["style-src", ["'self'", "'unsafe-inline'", ...resource]],
    ["connect-src", or(csp.connect, "'none'")],
    ["img-src", ["'self'", "data:", ...resource]],
    ["font-src", ["'self'", ...resource]],
    ["media-src", ["'self'", "data:", ...resource]],
    ["frame-src", or(csp.frame, "'none'")],
    ["object-src", ["'none'"]],
    ["base-uri", or(csp.baseUri, "'self'")],
    // Not in the spec's list; a further restriction it allows. An app has no
    // business submitting a form anywhere: it talks through the bridge.
    ["form-action", ["'none'"]],
  ]
  return directives.map(([name, sources]) => `${name} ${sources.join(" ")}`).join("; ")
}

/** The applied domains as MCP Apps spells them (`McpUiResourceCsp`), for the app to read. */
export function approvedDomains(csp: AppliedCsp): JsonObject {
  const named: [string, readonly CspSource[]][] = [
    ["connectDomains", csp.connect],
    ["resourceDomains", csp.resource],
    ["frameDomains", csp.frame],
    ["baseUriDomains", csp.baseUri],
  ]
  return Object.fromEntries(
    named
      .filter(([, sources]) => sources.length > 0)
      .map(([key, sources]) => [key, spell(sources)]),
  )
}

/**
 * The method the reporter in an app's document tells the proxy of a blocked
 * load with, in the namespace MCP Apps reserves for host ↔ proxy.
 */
export const cspViolationMethod = "ui/notifications/sandbox-csp-violation"

/**
 * Runs first in the app's document, before any of its own markup: tells the
 * proxy of each load the policy blocked, by the blocked origin alone. It
 * listens in capture on the window, so it hears a violation before anything
 * the app registers can; the app can send a report of its own, which says
 * only something about itself. Enforcement never depends on it.
 */
const reporter = `(function () {
  var parentWindow = window.parent;
  var post = parentWindow.postMessage.bind(parentWindow);
  window.addEventListener("securitypolicyviolation", function (event) {
    var origin;
    try { origin = new URL(event.blockedURI).origin; } catch (error) { origin = undefined; }
    post({ jsonrpc: "2.0", method: ${JSON.stringify(cspViolationMethod)}, params: origin && origin !== "null" ? { origin: origin } : {} }, "*");
  }, true);
})();`

/**
 * The document the proxy loads into the app's frame: the policy, as the
 * first element of the document so it governs everything after it, then the
 * reporter, then the app's own HTML. The policy is written from parsed parts
 * and keywords alone, so it holds no `"`, `<` or `&` to escape.
 *
 * The leading doctype keeps the document in standards mode; an app that
 * meant quirks mode does not get it.
 */
export function appDocument(html: string, csp: AppliedCsp): string {
  return (
    `<!doctype html><meta http-equiv="Content-Security-Policy" content="${cspPolicy(csp)}">` +
    `<script>${reporter}</script>` +
    html
  )
}
