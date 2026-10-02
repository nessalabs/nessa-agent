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
  /**
   * The host → the proxy: the app's document, the policy for the proxy's
   * own, and how long the app's frame has, after each of its loads, to
   * answer `appCheck` (`checkWithin`, a whole number of milliseconds a timer
   * can wait: at most 2^31 − 1).
   */
  resourceReady: "ui/notifications/sandbox-resource-ready",
  /** The reporter or the proxy → the host: a load the policy refused, by its origin. */
  cspViolation: "ui/notifications/sandbox-csp-violation",
  /**
   * The proxy → the host: the app's document is gone — another document
   * named itself, or a load went unanswered (`appCheck`) — or its reporter →
   * the proxy: it is going, on `pagehide`.
   */
  appLeft: "ui/notifications/sandbox-app-left",
  /**
   * The reporter → the proxy, at its start and in answer: the frame's token,
   * which document it is (an id the document cannot read), and the check it
   * answers. The proxy → the reporter, at every `load` of the frame, with
   * the check's number: which document is there now? The first named is
   * pinned; another named, or the latest check unanswered in time, is a
   * departure. Never relayed either way.
   */
  appCheck: "ui/notifications/sandbox-app-check",
} as const

/**
 * Where, in the reporter, the proxy writes the token it mints for the app's
 * frame (`sandbox/proxy.html`): the check's answer and the departure notice
 * carry it, so the proxy takes them from the app's frame alone — the notice
 * even after its document is gone, when the browser no longer names the
 * sender. Another frame cannot read it; the app can, so it says the app's
 * frame, not which of the app's documents.
 */
export const frameTokenSlot = "__nessa_frame_token__"

/**
 * The prefix of every method above: the proxy relays none from the host to
 * the app, and of the app's only `cspViolation`.
 */
export const sandboxPrefix = "ui/notifications/sandbox-"
