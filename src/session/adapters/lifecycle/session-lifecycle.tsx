import { useEffect, useRef } from "react"
import type { NessaClient } from "@nessa/client"

import { connectDevSession } from "../client/dev-session"
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
  const clientRef = useRef<NessaClient | null>(null)
  const reconnectsRef = useRef(0)

  useEffect(() => {
    let cancelled = false
    let offClose: (() => void) | undefined

    async function open() {
      dispatch(sessionConnecting())
      try {
        const established = await connectDevSession()
        if (cancelled) {
          established.client.close()
          return
        }
        clientRef.current = established.client
        dispatch(sessionReady({ hello: established.hello, health: established.health }))
        offClose = established.client.onClose(() => {
          if (cancelled) return
          clientRef.current = null
          dispatch(sessionDisconnected())
          if (reconnectsRef.current < 1) {
            reconnectsRef.current += 1
            void open()
          }
        })
      } catch (error) {
        if (cancelled) return
        clientRef.current = null
        const message = error instanceof Error ? error.message : "Failed to connect"
        dispatch(sessionError(`${message}. Is nessa-server running? (just server)`))
      }
    }

    void open()

    return () => {
      cancelled = true
      offClose?.()
      clientRef.current?.close()
      clientRef.current = null
    }
  }, [dispatch])

  return null
}
