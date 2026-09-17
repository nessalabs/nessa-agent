import { isRetryableConnectionError, NessaConnectionClosedError } from "@nessa/client"
import { SessionHealthError, type EstablishedDevSession } from "../client/dev-session"
import type { createSessionHandle } from "../client/handle"

/** Keeps the application connected after the client's finite retry window ends.
 * Only transport setup is retried; no conversation commands are retained here.
 */
export function superviseSession({
  session,
  connect,
  connecting,
  reconnecting,
  ready,
  failed,
  onTerminalFailure,
}: {
  session: ReturnType<typeof createSessionHandle>
  connect: () => Promise<EstablishedDevSession>
  connecting: () => void
  reconnecting: () => void
  ready: (established: EstablishedDevSession) => void
  failed: (message: string) => void
  onTerminalFailure?: (error: unknown) => void
}): () => void {
  let disposed = false
  let generation = 0
  let attempts = 0
  let hasConnected = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let unsubscribe = () => {}
  let current: EstablishedDevSession["client"] | undefined

  function pending() {
    if (hasConnected) reconnecting()
    else connecting()
  }

  function failure(error: unknown, gen: number) {
    if (disposed || gen !== generation) return
    generation += 1
    unsubscribe()
    session.set(null)
    current?.close()
    current = undefined
    const cause = error instanceof SessionHealthError ? error.cause : error
    if (isRetryableConnectionError(cause)) {
      pending()
      const delay = Math.max(
        Math.min(500 * 2 ** Math.min(attempts++, 4), 5_000),
        cause instanceof NessaConnectionClosedError ? (cause.retryAfterMs ?? 0) : 0,
      )
      timer = setTimeout(() => {
        timer = undefined
        void open()
      }, delay)
    } else {
      failed(error instanceof Error ? error.message : "Could not connect to the gateway.")
      onTerminalFailure?.(cause)
    }
  }

  async function open() {
    const gen = ++generation
    pending()
    try {
      const established = await connect()
      if (disposed || gen !== generation) {
        established.client.close()
        return
      }
      current = established.client
      // onClose can fire synchronously for an already-closed client.
      const offClose = current.onClose((error) => failure(error, gen))
      if (disposed || gen !== generation) {
        offClose()
        return
      }
      const offState = current.onConnectionStateChange((state) => {
        if (disposed || gen !== generation) return
        if (state.status === "reconnecting") {
          session.set(null)
          reconnecting()
        } else if (state.status === "connected") {
          session.set(established.client)
          attempts = 0
          hasConnected = true
          ready({ ...established, hello: established.client.productSession })
        }
      })
      unsubscribe = () => {
        offClose()
        offState()
      }
      if (current.connectionState.status === "connected") {
        attempts = 0
        hasConnected = true
        session.set(current)
        ready({ ...established, hello: current.productSession })
      }
    } catch (error) {
      failure(error, gen)
    }
  }

  session.set(null)
  void open()
  return () => {
    disposed = true
    generation += 1
    clearTimeout(timer)
    unsubscribe()
    current?.close()
    session.set(null)
  }
}
