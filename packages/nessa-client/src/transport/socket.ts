import { RetryableConnectError } from "../application/connect-retry.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"

const OPEN_TIMEOUT_MS = 10_000

/** Wait until the WebSocket is open, or reject on error, close, or timeout. */
export function waitForSocketOpen(
  socket: WebSocket,
  timeoutMs = OPEN_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<void> {
  if (socket.readyState === WebSocket.OPEN) return Promise.resolve()

  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timeout)
      signal?.removeEventListener("abort", onAbort)
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
      reject(new RetryableConnectError("WebSocket connection failed"))
    }
    const onClose = (event: CloseEvent) => {
      cleanup()
      reject(
        new NessaConnectionClosedError(
          event?.code ?? 1006,
          "WebSocket closed before open",
        ),
      )
    }
    const onAbort = () => {
      cleanup()
      reject(new Error("WebSocket opening cancelled"))
    }
    const timeout = setTimeout(() => {
      cleanup()
      reject(new RetryableConnectError("WebSocket open timeout"))
    }, timeoutMs)

    socket.addEventListener("open", onOpen)
    socket.addEventListener("error", onError)
    socket.addEventListener("close", onClose)
    signal?.addEventListener("abort", onAbort, { once: true })
    if (signal?.aborted) onAbort()
  })
}
