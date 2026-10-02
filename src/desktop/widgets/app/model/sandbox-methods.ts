/**
 * The methods the host and its sandbox proxy speak, in the namespace MCP Apps
 * reserves for them (`ui/notifications/sandbox-`; spec, *Sandbox proxy*). The
 * one statement of each name: the host reads and sends them from here, the
 * proxy and the reporter in the app's document speak them, and
 * `sandbox/serve.test.ts` holds the proxy to every one, and to the prefix.
 */
export const sandboxMethods = {
  /** The proxy → the host: ready for the document. */
  proxyReady: "ui/notifications/sandbox-proxy-ready",
  /** The host → the proxy: the app's document, and the policy for the proxy's own. */
  resourceReady: "ui/notifications/sandbox-resource-ready",
  /** The reporter or the proxy → the host: a load the policy refused, by its origin. */
  cspViolation: "ui/notifications/sandbox-csp-violation",
  /**
   * The proxy → the host: the app's document is gone — the frame loaded
   * again, or did not answer `appCheck` — or its reporter → the proxy: it
   * is going, on `pagehide`.
   */
  appLeft: "ui/notifications/sandbox-app-left",
  /**
   * The proxy → the reporter, at the frame's first `load`, and back with the
   * frame's token: the document loaded is the one handed over. No answer is a
   * departure. Never relayed either way.
   */
  appCheck: "ui/notifications/sandbox-app-check",
} as const

/**
 * Where, in the reporter, the proxy writes the token it mints for the app's
 * frame (`sandbox/proxy.html`): the check's answer and the departure notice
 * carry it, so the proxy takes them from the document it handed over — the
 * notice even after that document is gone, when the browser no longer names
 * the sender — and from no other.
 */
export const departureTokenSlot = "__nessa_departure_token__"

/**
 * The prefix of every method above: the proxy relays none from the host to
 * the app, and of the app's only `cspViolation`.
 */
export const sandboxPrefix = "ui/notifications/sandbox-"
