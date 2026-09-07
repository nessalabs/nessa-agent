import type { ProductSessionReady } from "../protocol/product-types.js"
import type { SessionTransport } from "./session-port.js"
import type { NessaClientConfig } from "./client-config.js"
import { NessaConnectionClosedError } from "./connection-closed-error.js"
import { isRetryableConnectionError } from "./connect-retry.js"

/** Client lifecycle snapshot. Narrow on status: connected permits RPCs, reconnecting exposes the attempt and last transport error, and closed exposes the final error. Recovery never replays RPCs. */
export type ConnectionState =
  | { status: "connected" }
  | { status: "reconnecting"; attempt: number; error: NessaConnectionClosedError }
  | { status: "closed"; error: Error }

export type ConnectedSession = {
  wire: SessionTransport
  ready: ProductSessionReady
  profile: "product"
}

/** RPCs issued during recovery fail immediately; they are never queued or replayed. */
export class NessaSessionUnavailableError extends Error {
  constructor() {
    super("Session is not connected")
    this.name = "NessaSessionUnavailableError"
  }
}

/** Owns transport replacement and persistent subscriptions; dependencies are injected. */
export class ManagedSession {
  private current: ConnectedSession | undefined
  private readonly lifetime = new AbortController()
  private readonly events = new Set<{
    event: string
    handler: (payload: unknown) => void
    off?: () => void
  }>()
  private readonly observers = new Set<(state: ConnectionState) => void>()
  private readonly closers = new Set<(error: Error) => void>()
  private value: ConnectionState = { status: "connected" }

  constructor(
    initial: ConnectedSession,
    private readonly config: NessaClientConfig,
    private readonly connect: (signal: AbortSignal) => Promise<ConnectedSession>,
    private readonly timing: {
      wait(ms: number, signal: AbortSignal): Promise<void>
      random(): number
    },
  ) {
    this.adopt(initial)
  }

  get state(): ConnectionState {
    return this.value
  }
  get ready(): ProductSessionReady | undefined {
    return this.current?.ready
  }

  request(method: string, params: unknown): Promise<unknown> {
    return (
      this.current?.wire.request(method, params) ??
      Promise.reject(new NessaSessionUnavailableError())
    )
  }

  onEvent(event: string, handler: (payload: unknown) => void): () => void {
    if (this.value.status === "closed") return () => {}
    const subscription = {
      event,
      handler,
      off: this.current?.wire.onEvent(event, handler),
    }
    this.events.add(subscription)
    return () => {
      subscription.off?.()
      this.events.delete(subscription)
    }
  }

  onState(handler: (state: ConnectionState) => void): () => void {
    if (this.value.status === "closed") return () => {}
    this.observers.add(handler)
    return () => {
      this.observers.delete(handler)
    }
  }

  onClose(handler: (error: Error) => void): () => void {
    if (this.value.status === "closed") {
      handler(this.value.error)
      return () => {}
    }
    this.closers.add(handler)
    return () => {
      this.closers.delete(handler)
    }
  }

  close(): void {
    this.finish(new NessaConnectionClosedError(1000, "client closed connection"))
  }

  private publish(state: ConnectionState): void {
    this.value = Object.freeze(state)
    for (const observer of [...this.observers]) {
      try {
        observer(state)
      } catch {
        /* Observers cannot interrupt cleanup or recovery. */
      }
    }
  }

  private adopt(session: ConnectedSession): void {
    if (this.lifetime.signal.aborted) {
      session.wire.close()
      return
    }
    this.current = session
    for (const subscription of this.events)
      subscription.off = session.wire.onEvent(subscription.event, subscription.handler)
    const disconnected = (error: NessaConnectionClosedError) => {
      if (this.current !== session) return
      this.current = undefined
      for (const subscription of this.events) {
        subscription.off?.()
        subscription.off = undefined
      }
      if (
        session.profile === "product" &&
        error.retryable &&
        this.config.reconnect.enabled
      ) {
        void this.recover(error)
      } else this.finish(error)
    }
    session.wire.onClose(disconnected)
    if (session.wire.termination) {
      // A close between handshake completion and adoption must never look connected.
      disconnected(session.wire.termination)
    } else this.publish({ status: "connected" })
  }

  private async recover(cause: NessaConnectionClosedError): Promise<void> {
    const policy = this.config.reconnect
    let last: Error = cause
    for (let attempt = 1; attempt <= policy.maxAttempts; attempt++) {
      if (this.lifetime.signal.aborted) return
      this.publish({ status: "reconnecting", attempt, error: cause })
      const ceiling = Math.min(
        policy.maxDelayMs,
        policy.initialDelayMs * 2 ** Math.min(attempt - 1, 31),
      )
      const delay = policy.jitter
        ? ceiling / 2 + (this.timing.random() * ceiling) / 2
        : ceiling
      try {
        await this.timing.wait(
          Math.max(
            delay,
            last instanceof NessaConnectionClosedError ? (last.retryAfterMs ?? 0) : 0,
          ),
          this.lifetime.signal,
        )
        if (this.lifetime.signal.aborted) return
        const session = await this.connect(this.lifetime.signal)
        this.adopt(session)
        return
      } catch (error) {
        if (this.lifetime.signal.aborted) return
        last = error instanceof Error ? error : new Error("Reconnection failed")
        if (!isRetryableConnectionError(last)) break
      }
    }
    this.finish(last)
  }

  private finish(error: Error): void {
    if (this.lifetime.signal.aborted) return
    this.lifetime.abort()
    const wire = this.current?.wire
    this.current = undefined
    for (const subscription of this.events) subscription.off?.()
    this.events.clear()
    wire?.close()
    this.publish({ status: "closed", error })
    this.observers.clear()
    for (const observer of [...this.closers]) {
      try {
        observer(error)
      } catch {
        /* Preserve final notification for other observers. */
      }
    }
    this.closers.clear()
  }
}
