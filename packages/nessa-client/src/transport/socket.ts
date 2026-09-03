const OPEN_TIMEOUT_MS = 10_000

/** Wait until the WebSocket is open, or reject on error, close, or timeout. */
export function waitForSocketOpen(
  socket: WebSocket,
  timeoutMs = OPEN_TIMEOUT_MS,
): Promise<void> {
  if (socket.readyState === WebSocket.OPEN) return Promise.resolve()

  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timeout)
      socket.removeEventListener("open", onOpen)
      socket.removeEventListener("error", onError)
      socket.removeEventListener("close", onClose)
    }

    const onOpen = () => {
      cleanup()
      resolve()
    }
    const onError = () => {
      cleanup()
      reject(new Error("WebSocket connection failed"))
    }
    const onClose = () => {
      cleanup()
      reject(new Error("WebSocket closed before open"))
    }
    const timeout = setTimeout(() => {
      cleanup()
      reject(new Error("WebSocket open timeout"))
    }, timeoutMs)

    socket.addEventListener("open", onOpen)
    socket.addEventListener("error", onError)
    socket.addEventListener("close", onClose)
  })
}
