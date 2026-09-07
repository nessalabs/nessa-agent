import { useEffect } from "react"

import { SessionHealthError } from "../client/dev-session"
import type { createSessionHandle } from "../client/handle"
import type { connectDevSession } from "../client/dev-session"
import {
  sessionConnecting,
  sessionDisconnected,
  sessionError,
  sessionReady,
} from "../store/slice"
import { useSessionDispatch } from "../store/hooks"

/**
 * Owns the WebSocket client lifecycle. Mounted once from the composition root.
 * The SDK owns reconnection; terminal authentication failures remain terminal.
 */
export function SessionLifecycle({
  dependencies,
}: {
  dependencies: {
    session: ReturnType<typeof createSessionHandle>
    connectSession: typeof connectDevSession
  }
}) {
  const dispatch = useSessionDispatch()

  useEffect(() => {
    let cancelled = false
    let offClose: (() => void) | undefined
    let offState: (() => void) | undefined
    let generation = 0

    async function open() {
      const gen = ++generation
      dependencies.session.set(null)
      dispatch(sessionConnecting())
      try {
        const established = await dependencies.connectSession()
        if (cancelled || gen !== generation) {
          established.client.close()
          return
        }

        // Subscribe before publishing ready so a fast close cannot leave Redux
        // stuck on `ready` with no reconnect path.
        offClose?.()
        offClose = established.client.onClose(() => {
          if (cancelled || gen !== generation) return
          dependencies.session.set(null)
          dispatch(sessionDisconnected())
        })

        offState?.()
        offState = established.client.onConnectionStateChange((state) => {
          if (cancelled || gen !== generation) return
          if (state.status === "reconnecting") dispatch(sessionConnecting())
          if (state.status === "connected")
            dispatch(
              sessionReady({
                hello: established.client.productSession,
                health: established.health,
              }),
            )
        })
        if (established.client.connectionState.status === "closed") return
        dependencies.session.set(established.client)
        if (established.client.connectionState.status === "reconnecting") {
          dispatch(sessionConnecting())
          return
        }
        dispatch(sessionReady({ hello: established.hello, health: established.health }))
      } catch (error) {
        if (cancelled || gen !== generation) return
        dependencies.session.set(null)
        const message = error instanceof Error ? error.message : "Failed to connect"
        const hint =
          error instanceof SessionHealthError
            ? "Session opened but health check failed."
            : "Is nessa-server running? (just server)"
        dispatch(sessionError(`${message}. ${hint}`))
      }
    }

    void open()

    return () => {
      cancelled = true
      generation += 1
      offClose?.()
      offState?.()
      offClose = undefined
      dependencies.session.get()?.close()
      dependencies.session.set(null)
    }
  }, [dispatch, dependencies])

  return null
}
