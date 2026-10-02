/**
 * The methods the host and its sandbox proxy speak, in the namespace MCP Apps
 * reserves for them (`ui/notifications/sandbox-`; spec, *Sandbox proxy*). The
 * one statement of each name: the host reads and sends them from here, the
 * proxy and the reporter in the app's document speak them, and
 * `sandbox/serve.test.ts` holds the proxy to every one.
 */
export const sandboxMethods = {
  /** The proxy → the host: ready for the document. */
  proxyReady: "ui/notifications/sandbox-proxy-ready",
  /** The host → the proxy: the app's document, and the policy for the proxy's own. */
  resourceReady: "ui/notifications/sandbox-resource-ready",
  /** The reporter or the proxy → the host: a load the policy refused, by its origin. */
  cspViolation: "ui/notifications/sandbox-csp-violation",
  /** The reporter → the proxy → the host: the app's document is going, by any way. */
  appLeft: "ui/notifications/sandbox-app-left",
} as const

/**
 * Where, in the reporter, the proxy writes the token it mints for the app's
 * frame (`sandbox/proxy.html`): the departure notice carries it, so the proxy
 * takes it from the frame's own document even after that document is gone —
 * when the browser no longer names the sender — and from no other.
 */
export const departureTokenSlot = "__nessa_departure_token__"

/** The prefix of every method only the host and its proxy speak. */
export const sandboxPrefix = "ui/notifications/sandbox-"
