/** Exact browser socket evidence for a stress step's intentional gateway restart. */
const causes = new Map([
  ["Error in connection establishment: net::ERR_CONNECTION_REFUSED", "refused"],
  ["net::ERR_CONNECTION_REFUSED", "refused"],
  ["Could not connect to 127.0.0.1: Connection refused", "refused"],
  ["Error receiving data: Connection reset by peer", "reset"],
])
const MAX_EVENTS = 64

/**
 * Install before navigation, so an existing socket's reset retains its URL.
 * The page-line owner still reports every line; no future exemption is installed.
 */
export function observeGatewayRestarts(page, endpoint) {
  const url = new URL("/session", endpoint).href
  const prefix = `WebSocket connection to '${url}' failed: `
  const sockets = new Map()
  let active = null
  let disposed = false
  let overflow = false
  const onSocket = (socket) => {
    if (socket.url() !== url) return
    if (sockets.size === MAX_EVENTS) {
      overflow = true
      return
    }
    const born = active
    let failed = false
    const error = (text) => {
      failed = true
      if (!active) return
      active.recovery = null
      const cause = causes.get(text)
      if (cause) {
        if (active.errors.length === MAX_EVENTS) active.invalid = true
        else active.errors.push(cause)
      }
    }
    const received = () => {
      if (!failed && active && active === born) active.recovery = socket
    }
    const close = () => {
      socket.off("socketerror", error)
      socket.off("framereceived", received)
      socket.off("close", close)
      sockets.delete(socket)
    }
    sockets.set(socket, close)
    socket.on("socketerror", error)
    socket.once("framereceived", received)
    socket.once("close", close)
  }
  page.on("websocket", onSocket)
  return {
    /** `action` returns true only after the existing recovery assertions hold. */
    async during(opened, action) {
      if (disposed || active || opened.page !== page)
        throw new Error("gateway restart observation is not available")
      const prior = new Set()
      let priorOverflow = false
      opened.reclassifyHeld(
        (line) => {
          if (!prior.has(line)) {
            if (prior.size === MAX_EVENTS) priorOverflow = true
            else prior.add(line)
          }
          return false
        },
        (line) => line,
      )
      const scope = {
        errors: [],
        lines: [],
        recovery: null,
        invalid: overflow || priorOverflow,
      }
      const consoleError = (message) => {
        if (message.type() !== "error") return
        const text = message.text()
        if (!text.startsWith(prefix)) return
        const cause = causes.get(text.slice(prefix.length))
        if (!cause) return
        scope.recovery = null
        const location = message.location()?.url ?? ""
        if (scope.lines.length === MAX_EVENTS) {
          scope.invalid = true
          return
        }
        scope.lines.push({
          cause,
          line: `console.error: ${text.slice(0, 300)}${location ? ` (${location})` : ""}`,
        })
      }
      active = scope
      page.on("console", consoleError)
      let verified
      try {
        verified = await action()
      } finally {
        page.off("console", consoleError)
        active = null
      }
      if (
        disposed ||
        overflow ||
        verified !== true ||
        !sockets.has(scope.recovery) ||
        scope.invalid
      )
        return
      const available = new Map()
      for (const cause of scope.errors)
        available.set(cause, (available.get(cause) ?? 0) + 1)
      const eligible = new Map()
      for (const { cause, line } of scope.lines) {
        const count = available.get(cause) ?? 0
        if (count === 0) continue
        available.set(cause, count - 1)
        eligible.set(line, (eligible.get(line) ?? 0) + 1)
      }
      opened.reclassifyHeld(
        (line) => {
          if (prior.has(line)) return false
          const count = eligible.get(line) ?? 0
          if (!count) return false
          eligible.set(line, count - 1)
          return true
        },
        (line) => `${line} [owned gateway restart; session reconnected]`,
      )
    },
    dispose() {
      disposed = true
      page.off("websocket", onSocket)
      for (const close of sockets.values()) close()
    },
  }
}
