import { RetryableConnectError } from "../application/connect-retry.js"
import {
  encodeWireMessage,
  isEventFrame,
  isResponseFrame,
  parseWireMessage,
  type Frame,
} from "../protocol/index.js"
import { buildRequestFrame } from "../protocol/encode.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"

const DEFAULT_REQUEST_TIMEOUT_MS = 30_000

type PendingRequest = {
  resolve: (payload: unknown) => void
  reject: (error: Error) => void
  timer: ReturnType<typeof setTimeout>
}

/**
 * WebSocket transport: correlates req/res pairs and dispatches events.
 * Repository layer — knows wire I/O, not connect policy or public API shape.
 */
export class WireSession {
  private readonly pending = new Map<string, PendingRequest>()
  private readonly eventListeners = new Map<string, Set<(payload: unknown) => void>>()
  private readonly closeListeners = new Set<(error: NessaConnectionClosedError) => void>()
  private closedError: NessaConnectionClosedError | undefined
  private requestSeq = 0
  private readonly requestTimeoutMs: number

  constructor(
    private readonly socket: WebSocket,
    options?: { requestTimeoutMs?: number },
  ) {
    this.requestTimeoutMs = options?.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS
    // Node's ws adapter emits an error when a CONNECTING socket is closed.
    // Keep an error listener through teardown, after the open waiter is removed.
    // Close events or bounded request timers settle outstanding work.
    socket.addEventListener("error", () => {})
    socket.addEventListener("message", (event) => {
      this.handleWireMessage(String(event.data))
    })
    socket.addEventListener("close", (event) => {
      this.handleClose(event.code, event.reason)
    })
  }

  get termination(): NessaConnectionClosedError | undefined {
    return this.closedError
  }

  /** Send an RPC and await the matching response frame. */
  request(
    method: string,
    params: unknown,
    timeoutMs = this.requestTimeoutMs,
  ): Promise<unknown> {
    if (this.closedError) return Promise.reject(this.closedError)
    if (this.socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new RetryableConnectError("WebSocket is not open"))
    }

    const id = String(++this.requestSeq)
    const frame = buildRequestFrame(id, method, params)

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return
        reject(new RetryableConnectError(`request timeout: ${method}`))
      }, timeoutMs)

      this.pending.set(id, {
        resolve: (payload) => {
          clearTimeout(timer)
          resolve(payload)
        },
        reject: (error) => {
          clearTimeout(timer)
          reject(error)
        },
        timer,
      })

      try {
        this.socket.send(encodeWireMessage(frame))
      } catch {
        clearTimeout(timer)
        this.pending.delete(id)
        reject(new RetryableConnectError("WebSocket send failed"))
      }
    })
  }

  /** Subscribe to a server push event by name. */
  onEvent(event: string, handler: (payload: unknown) => void): () => void {
    const set = this.eventListeners.get(event) ?? new Set()
    set.add(handler)
    this.eventListeners.set(event, set)
    return () => set.delete(handler)
  }

  onClose(handler: (error: NessaConnectionClosedError) => void): () => void {
    this.closeListeners.add(handler)
    return () => this.closeListeners.delete(handler)
  }

  /** @internal Test seam — inject a parsed frame without a socket. */
  dispatchFrame(frame: Frame): void {
    if (isEventFrame(frame)) {
      this.dispatchEvent(frame.event, frame.payload)
      return
    }

    if (!isResponseFrame(frame)) return

    const pending = this.pending.get(frame.id)
    if (!pending) return

    this.pending.delete(frame.id)
    clearTimeout(pending.timer)
    if (frame.ok) {
      pending.resolve(frame.payload)
      return
    }

    const error = frame.error as
      { code?: string; message?: string; details?: unknown } | undefined
    pending.reject(
      new NessaRpcError(
        error?.code ?? "request_failed",
        error?.message ?? "request failed",
        error?.details,
      ),
    )
  }

  close(): void {
    this.handleClose(1000, "client closed connection")
    this.socket.close()
  }

  private handleWireMessage(raw: string): void {
    const frame = parseWireMessage(raw)
    if (!frame) return
    this.dispatchFrame(frame)
  }

  private dispatchEvent(event: string, payload: unknown): void {
    const handlers = this.eventListeners.get(event)
    if (!handlers) return
    for (const handler of handlers) handler(payload)
  }

  /** @internal Test seam for close behavior and pending-request cleanup. */
  dispatchClose(code = 1006, reason = ""): void {
    this.handleClose(code, reason)
  }

  private handleClose(code: number, reason: string): void {
    if (this.closedError) return
    this.closedError = new NessaConnectionClosedError(code, reason)
    const observers = [...this.closeListeners]
    this.eventListeners.clear()
    this.closeListeners.clear()
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer)
      pending.reject(new NessaConnectionClosedError(code, reason))
      this.pending.delete(id)
    }
    // Finish internal cleanup first. Consumer callbacks cannot keep other
    // requests or close observers alive, even if they throw or close again.
    for (const handler of observers) {
      try {
        handler(this.closedError)
      } catch {
        // Observer failures must not interrupt transport teardown.
      }
    }
  }
}
