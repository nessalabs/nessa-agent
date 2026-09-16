import { useEffect } from "react"
import type { createSessionHandle } from "../client/handle"
import type { connectDevSession } from "../client/dev-session"
import {
  sessionConnecting,
  sessionError,
  sessionReady,
  sessionReconnecting,
} from "../store/slice"
import { useSessionDispatch, useSessionSelector } from "../store/hooks"
import { superviseSession } from "./supervisor"

/** React owns the supervisor lifetime; the SDK owns retries within each client. */
export function SessionLifecycle({
  dependencies,
  onTerminalFailure,
}: {
  onTerminalFailure?: (error: unknown) => void
  dependencies: {
    session: ReturnType<typeof createSessionHandle>
    connectSession: typeof connectDevSession
  }
}) {
  const dispatch = useSessionDispatch()
  const retryRequest = useSessionSelector((state) => state.session.retryRequest)
  useEffect(
    () =>
      superviseSession({
        session: dependencies.session,
        onTerminalFailure,
        connect: dependencies.connectSession,
        connecting: () => {
          dispatch(sessionConnecting())
        },
        reconnecting: () => {
          dispatch(sessionReconnecting())
        },
        failed: (message) => {
          dispatch(sessionError(message))
        },
        ready: ({ hello, health }) => {
          dispatch(sessionReady({ hello, health }))
        },
      }),
    [dispatch, dependencies, retryRequest, onTerminalFailure],
  )
  return null
}
