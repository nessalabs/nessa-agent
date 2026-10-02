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
 * `frameDomains` is never applied: `frame-src` is always `'none'`, so the
 * app's frame loads nothing but the document handed over — no nested frame
 * loads anything but inline content, and no navigation of its own frame
 * goes anywhere (#349, design amendment after review round 2). The app is told: the domains it is told were approved
 * never list any.
 *
 * The one owner of the policy: the host writes it (`cspPolicy`), into the
 * app's document (`appDocument`) and into the proxy's hands
 * (`sandbox-resource-ready`), and the proxy applies it to its own document as
 * given (`sandbox/proxy.html`).
 */
import { field, isObject, type Json, type JsonObject } from "./json-rpc"
import { frameTokenSlot, sandboxMethods } from "./sandbox-methods"

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
    // Never a declared domain: see the module's documentation.
    ["frame-src", ["'none'"]],
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
    ["baseUriDomains", csp.baseUri],
  ]
  return Object.fromEntries(
    named
      .filter(([, sources]) => sources.length > 0)
      .map(([key, sources]) => [key, spell(sources)]),
  )
}

/**
 * Runs first in the app's document, before any of its own markup, with its
 * listeners registered before any of the app's. While the app leaves them in
 * place — `document.open()` erases them, and a frame the app makes has none —
 * it:
 *
 * - says which document it is (`appCheck`): at once, before any of the
 *   app's scripts, and again in answer to each check the proxy sends at a
 *   `load` of the frame, naming the check it answers, which it keeps from
 *   the app. The id is minted here
 *   and held in this closure alone, never in the document, so a document
 *   that is not this one cannot give it;
 * - reports each load the policy blocked, by the blocked origin alone;
 * - says the document is going, on `pagehide`;
 * - keeps a link to a fragment of this document in it: an `about:srcdoc`
 *   document resolves `href="#x"` against the proxy's URL, a navigation the
 *   policy refuses and the proxy reads as the app's departure, so on a
 *   primary click nothing the app ran prevented, on an `<a>`, `<area>` or
 *   SVG `<a>` (found along the event's composed path) with no other target
 *   and no `download`, whose URL is the document's base but for its
 *   fragment, it moves to that fragment (`location.hash`) instead.
 *   Navigating to `"#x"` by script (`location.href`, `location.assign`)
 *   goes past it: an app sets `location.hash`.
 *
 * Its messages carry the token the proxy wrote into its slot
 * (`frameTokenSlot`), in this document alone: what says they come from the
 * app's frame and not an error page, a blank page or another frame. The app
 * can read the token too; it speaks for itself.
 *
 * The proxy relies on none of it to know the app is gone, only on its
 * answers (design L32): a document naming itself as another than the first,
 * or a `load` of the frame its document does not answer. It answers only a
 * check the browser delivered from the proxy, read through what it took
 * before the app ran, so the app cannot have it answer a check of the app's
 * own making, nor hear the proxy's. The app can send a report or a
 * departure itself; a report says only an origin, and a departure only ends
 * its own view. Enforcement never depends on them.
 */
const reporter = `(function () {
  var token = ${JSON.stringify(frameTokenSlot)};
  var parentWindow = window.parent;
  var post = parentWindow.postMessage.bind(parentWindow);
  // Taken now, before any of the app's scripts can replace them, and called
  // through \`apply\`, never looked up on the event again.
  var apply = Reflect.apply;
  var sourceOf = Object.getOwnPropertyDescriptor(MessageEvent.prototype, "source").get;
  var dataOf = Object.getOwnPropertyDescriptor(MessageEvent.prototype, "data").get;
  var stopImmediately = Event.prototype.stopImmediatePropagation;
  var bytes = crypto.getRandomValues(new Uint8Array(16));
  var documentId = "";
  for (var i = 0; i < bytes.length; i++) documentId += (bytes[i] + 256).toString(16).slice(1);
  function here(check) {
    post({ jsonrpc: "2.0", method: ${JSON.stringify(sandboxMethods.appCheck)}, params: { token: token, document: documentId, check: check } }, "*");
  }
  window.addEventListener("securitypolicyviolation", function (event) {
    var origin;
    try { origin = new URL(event.blockedURI).origin; } catch (error) { origin = undefined; }
    post({ jsonrpc: "2.0", method: ${JSON.stringify(sandboxMethods.cspViolation)}, params: origin && origin !== "null" ? { origin: origin } : {} }, "*");
  }, true);
  window.addEventListener("message", function (event) {
    // Only a message the browser delivered (an event the app dispatches
    // itself is not trusted, and \`isTrusted\` is the event's own, past
    // the app's reach), from the proxy.
    if (event.isTrusted !== true || apply(sourceOf, event, []) !== parentWindow) return;
    var data = apply(dataOf, event, []);
    if (data === null || typeof data !== "object" || data.method !== ${JSON.stringify(sandboxMethods.appCheck)}) return;
    apply(stopImmediately, event, []);
    here(data.params !== null && typeof data.params === "object" ? data.params.check : undefined);
  }, true);
  window.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    var path = event.composedPath();
    var link = null;
    for (var at = 0; at < path.length && link === null; at++) {
      var node = path[at];
      if (node && (node.localName === "a" || node.localName === "area") && typeof node.getAttribute === "function") link = node;
    }
    if (link === null) return;
    var target = link.getAttribute("target");
    if ((target !== null && target !== "" && target.toLowerCase() !== "_self") || link.hasAttribute("download")) return;
    var href = link.getAttribute("href");
    if (href === null) href = link.getAttributeNS("http://www.w3.org/1999/xlink", "href");
    if (href === null || href.indexOf("#") === -1) return;
    var to, base;
    try { to = new URL(href, document.baseURI); base = new URL(document.baseURI); } catch (error) { return; }
    var fragment = to.hash;
    to.hash = "";
    base.hash = "";
    if (to.href !== base.href) return;
    event.preventDefault();
    location.hash = fragment;
  }, false);
  window.addEventListener("pagehide", function () {
    post({ jsonrpc: "2.0", method: ${JSON.stringify(sandboxMethods.appLeft)}, params: { token: token } }, "*");
  }, true);
  here();
})();`

/**
 * The document the proxy loads into the app's frame: the policy, as the
 * first element of the document so it governs everything after it, then the
 * reporter, then the app's own HTML. The policy is written from parsed parts
 * and keywords alone, so it holds no `"`, `<` or `&` to escape.
 *
 * The proxy applies the same policy to its own document before it makes the
 * frame (`sandbox-resource-ready`'s `policy`), and that is the copy that
 * holds the frame itself: a document's policy governs only what it loads,
 * so this one would be gone the moment the app navigated its frame
 * elsewhere, which the proxy's `frame-src 'none'` refuses. This one is kept
 * because the `srcdoc`'s inheriting the proxy's is not yet seen in WebKit.
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
