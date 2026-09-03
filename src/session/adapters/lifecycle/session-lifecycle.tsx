import { useEffect, useRef } from "react"

import { connectDevSession, SessionHealthError } from "../client/dev-session"
import { getSessionClient, setSessionClient } from "../client/handle"
import {
  sessionConnecting,
  sessionDisconnected,
  sessionError,
  sessionReady,
} from "../store/slice"
import { useSessionDispatch } from "../store/hooks"

/**
 * Owns the WebSocket client lifecycle. Mounted once from the composition root.
 * Reconnects at most once after an unexpected close.
 */
export function SessionLifecycle() {
  const dispatch = useSessionDispatch()
  const reconnectsRef = useRef(0)

  useEffect(() => {
    let cancelled = false
    let offClose: (() => void) | undefined
    let generation = 0

    async function open() {
      const gen = ++generation
      setSessionClient(null)
      dispatch(sessionConnecting())
      try {
        const established = await connectDevSession()
        if (cancelled || gen !== generation) {
          established.client.close()
          return
        }

        // Subscribe before publishing ready so a fast close cannot leave Redux
        // stuck on `ready` with no reconnect path.
        offClose?.()
        offClose = established.client.onClose(() => {
          if (cancelled || gen !== generation) return
          setSessionClient(null)
          dispatch(sessionDisconnected())
          if (reconnectsRef.current < 1) {
            reconnectsRef.current += 1
            void open()
          }
        })

        setSessionClient(established.client)
        dispatch(sessionReady({ hello: established.hello, health: established.health }))
      } catch (error) {
        if (cancelled || gen !== generation) return
        setSessionClient(null)
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
      offClose = undefined
      getSessionClient()?.close()
      setSessionClient(null)
    }
  }, [dispatch])

  return null
}
