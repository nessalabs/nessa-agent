/**
 * What a pane was showing, kept for the frames its box stays after the
 * source removes it. The layout drops the pane immediately and the grid
 * holds the box; the record is already gone, so the box would paint empty.
 * The snapshot stays with the component that read it. A later mount of the
 * same id reads the store, and finds nothing once the box has gone.
 *
 * A new session's draft goes the same way. `withPanes` lets it go in the
 * close, and the home would go with it while the box is still there. An
 * established session the source removes does too: `forgetSession` drops
 * its unsent reply, its outbox, a transcript read that failed, and the
 * transcript a turn was still running in. The composer, the message still
 * on its way, that failure, and Stop would clear while the box stays.
 * What the composer was showing, that message, the failure, and whether
 * the agent was at work stay with the component.
 */
import { useRef } from "react"
import type { Transcript } from "../../model/transcript"
import type { SessionSummary } from "../../model/workspace-index"
import { useWorkspaceSelector } from "./hooks"
import { selectDraft, selectSession, selectTranscript } from "./selectors"

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

/**
 * Whether this pane shows a new session's home. The live draft does. So
 * does a draft the close already let go, while its box stays and no listed
 * session has taken the id: the layout drops the record at once. A session
 * this pane started is listed, so the home goes with the record.
 */
export function useShownDraft(
  sessionId: string,
  liveSession: string,
  listed: boolean,
): boolean {
  const live = useWorkspaceSelector(
    (state) => selectDraft(state, sessionId) !== undefined,
  )
  const remembered = useRef<string | null>(null)
  if (live) remembered.current = sessionId
  if (live) return true
  return (
    liveSession === "" && !listed && remembered.current === sessionId && sessionId !== ""
  )
}

/**
 * The value last read while `sessionId` was a draft or a listed session,
 * for the frames after that record is gone and the box is still on screen.
 * A session this component has not shown keeps the live value.
 */
export function useRememberedValue<T>(sessionId: string, live: T): T {
  const present = useWorkspaceSelector(
    (state) =>
      selectDraft(state, sessionId) !== undefined ||
      selectSession(state, sessionId) !== undefined,
  )
  const remembered = useRef<{ id: string; value: T } | null>(null)
  if (present) remembered.current = { id: sessionId, value: live }
  if (present || remembered.current?.id !== sessionId) return live
  return remembered.current.value
}
