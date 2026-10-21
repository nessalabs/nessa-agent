/**
 * The session a pane was showing, kept for the frames its box stays after
 * the source removes it. The layout drops the pane immediately and the grid
 * holds the box; the record is already gone, so the box would paint empty.
 * The snapshot stays with the component that read it. A later mount of the
 * same id reads the store, and finds nothing once the box has gone.
 */
import { useRef } from "react"
import type { Transcript } from "../../model/transcript"
import type { SessionSummary } from "../../model/workspace-index"
import { useWorkspaceSelector } from "./hooks"
import { selectSession, selectTranscript } from "./selectors"

export function useRememberedSession(sessionId: string): SessionSummary | undefined {
  const live = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const remembered = useRef<{ id: string; session: SessionSummary } | null>(null)
  if (live) remembered.current = { id: sessionId, session: live }
  return (
    live ??
    (remembered.current?.id === sessionId ? remembered.current.session : undefined)
  )
}

export function useRememberedTranscript(sessionId: string): Transcript | undefined {
  const live = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const remembered = useRef<{ id: string; transcript: Transcript } | null>(null)
  if (live) remembered.current = { id: sessionId, transcript: live }
  return (
    live ??
    (remembered.current?.id === sessionId ? remembered.current.transcript : undefined)
  )
}
