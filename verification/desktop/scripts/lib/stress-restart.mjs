/** Exact browser socket evidence for a stress step's intentional gateway restart. */
const causes = new Map([
  ["Error in connection establishment: net::ERR_CONNECTION_REFUSED", "refused"],
  ["net::ERR_CONNECTION_REFUSED", "refused"],
  ["Could not connect to 127.0.0.1: Connection refused", "refused"],
  ["Could not connect to the server.", "refused"],
  ["Error receiving data: Connection reset by peer", "reset"],
  ["The network connection was lost.", "reset"],
])
const MAX_EVENTS = 64
const reads = new Set(["conversation.subscribeList", "conversation.subscribe"])

function frame(event) {
  try {
    return JSON.parse(String(event.payload))
  } catch {
    return null
  }
}

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
    const pending = new Set()
    const error = (text) => {
      failed = true
      if (!active) return
      active.recovery = null
      if (active.read === socket) active.read = null
      const cause = causes.get(text)
      if (cause) {
        if (active.errors.length === MAX_EVENTS) active.invalid = true
        else active.errors.push(cause)
      }
    }
    const sent = (event) => {
      if (!active || active !== born || failed) return
      const request = frame(event)
      if (
        request?.type !== "req" ||
        typeof request.id !== "string" ||
        !reads.has(request.method)
      )
        return
      if (pending.size === MAX_EVENTS) active.invalid = true
      else pending.add(request.id)
    }
    // Keep the console oracle's original first-frame proof separate: another
    // frame on this same socket cannot erase a later console failure.
    const recovered = () => {
      if (!failed && active && active === born) active.recovery = socket
    }
    const received = (event) => {
      if (!active || active !== born || failed) return
      const response = frame(event)
      if (response?.type !== "res" || !pending.delete(response.id)) return
      if (response.ok !== true || active.invalid) return
      // Workspace subscriptions are issued through the adopted managed client,
      // after authentication. A challenge or another Node client's health/list
      // cannot establish this browser's recovery (stress-restart.test.mjs).
      active.read = socket
      for (const finish of active.waiters) finish(true)
    }
    const close = () => {
      socket.off("socketerror", error)
      socket.off("framesent", sent)
      socket.off("framereceived", recovered)
      socket.off("framereceived", received)
      socket.off("close", close)
      sockets.delete(socket)
      if (active?.read === socket) active.read = null
    }
    sockets.set(socket, close)
    socket.on("socketerror", error)
    socket.on("framesent", sent)
    socket.once("framereceived", recovered)
    socket.on("framereceived", received)
    socket.once("close", close)
  }
  page.on("websocket", onSocket)
  return {
    /** A replacement browser workspace read answered within the caller's budget. */
    async readable(milliseconds) {
      const scope = active
      const current = () =>
        !disposed &&
        active === scope &&
        scope &&
        !scope.invalid &&
        sockets.has(scope.read)
      if (current()) return true
      if (!scope || disposed || milliseconds <= 0) return false
      const answered = await new Promise((resolve) => {
        const finish = (value) => {
          clearTimeout(timer)
          scope.waiters.delete(finish)
          resolve(value)
        }
        const timer = setTimeout(() => finish(false), milliseconds)
        scope.waiters.add(finish)
      })
      return answered === true && Boolean(current())
    },
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
        read: null,
        waiters: new Set(),
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
        for (const finish of scope.waiters) finish(false)
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
      if (active) for (const finish of active.waiters) finish(false)
      page.off("websocket", onSocket)
      for (const close of sockets.values()) close()
    },
  }
}
