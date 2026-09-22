import type { ProductSessionReady } from "../protocol/product-types.js"
import type { SessionTransport } from "./session-port.js"
import type { NessaClientConfig } from "./client-config.js"
import { NessaConnectionClosedError } from "./connection-closed-error.js"
import { isRetryableConnectionError } from "./connect-retry.js"

/** Client lifecycle snapshot. Narrow on status: connected permits RPCs, reconnecting exposes the attempt and the error that began recovery, and closed exposes the final error. Recovery never replays RPCs. */
export type ConnectionState =
  | { status: "connected" }
  | { status: "reconnecting"; attempt: number; error: NessaConnectionClosedError }
  | { status: "closed"; error: Error }

export type ConnectedSession = {
  wire: SessionTransport
  ready: ProductSessionReady
  profile: "product"
  /** Authenticated socket URL owned by this exact transport. */
  url: string
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
  private publication = 0

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
  get url(): string | undefined {
    return this.current?.url
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
    const publication = ++this.publication
    this.value = Object.freeze(state)
    for (const observer of [...this.observers]) {
      if (publication !== this.publication) break
      try {
        observer(state)
      } catch {
        /* Observers cannot interrupt cleanup or recovery. */
      }
    }
  }

  /**
   * Take ownership of `session` and move its subscriptions across.
   *
   * A replacement can already be closed when it arrives. Reported to a running
   * recovery (`recovering`), that is one spent attempt of its budget and the
   * loop decides what happens next; the termination is returned rather than
   * acted on here. Outside recovery there is no such loop, so the usual
   * disconnect path starts one.
   */
  private adopt(
    session: ConnectedSession,
    recovering = false,
  ): NessaConnectionClosedError | undefined {
    if (this.lifetime.signal.aborted) {
      session.wire.close()
      return undefined
    }
    this.current = session
    for (const subscription of this.events)
      subscription.off = session.wire.onEvent(subscription.event, subscription.handler)
    const release = () => {
      this.current = undefined
      for (const subscription of this.events) {
        subscription.off?.()
        subscription.off = undefined
      }
    }
    // A transport is free to report an existing termination the moment a close
    // handler is registered. `SessionTransport` does not forbid it, and a
    // handler that ran during registration would start recovery from inside
    // adoption — the nested loop with its own budget that this exists to stop.
    // Until adoption finishes, a close is remembered rather than acted on, and
    // the check below is the single place that decides what it meant.
    let adopted = false
    let during: NessaConnectionClosedError | undefined
    const disconnected = (error: NessaConnectionClosedError) => {
      if (this.current !== session) return
      if (!adopted) {
        during ??= error
        return
      }
      release()
      if (
        session.profile === "product" &&
        error.retryable &&
        this.config.reconnect.enabled
      ) {
        void this.recover(error)
      } else this.finish(error)
    }
    session.wire.onClose(disconnected)
    adopted = true
    const termination = session.wire.termination ?? during
    if (!termination) {
      this.publish({ status: "connected" })
      return undefined
    }
    // A close between handshake completion and adoption must never look connected.
    if (!recovering) {
      disconnected(termination)
      return undefined
    }
    if (this.current === session) release()
    // Adoption took this transport; refusing it means letting it go, the way
    // `finish` does for the one it was holding.
    session.wire.close()
    return termination
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
        // A replacement that never became connected does not end recovery, and
        // does not get a fresh budget either: it is this attempt's outcome.
        const termination = this.adopt(session, true)
        if (!termination) return
        if (this.lifetime.signal.aborted) return
        last = termination
        if (!isRetryableConnectionError(last)) break
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
