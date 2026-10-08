import type { RefObject } from "react"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel, selectSession } from "../../adapters/store/selectors"
import { revealSession } from "../../adapters/store/commands"
import { useNow } from "../../adapters/dom/clock"
import { startedLabel } from "../../model/time-labels"
import { useListedChannel, useWorkspaceFrame } from "../workspace-frame"
import { tooltip } from "../../../ui/tooltip"

/**
 * A conversation's heading: its title, and where and when it began, set
 * lightly at the top of the transcript and scrolling away with it. The
 * channel is left out when the session list beside already names it, and
 * reveals the session in the sidebar when chosen.
 */
export function TranscriptHeading({
  sessionId,
  titleRef,
}: {
  sessionId: string
  titleRef: RefObject<HTMLHeadingElement | null>
}) {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const channel = useWorkspaceSelector((state) =>
    session ? selectChannel(state, session.channelId) : undefined,
  )
  const listed = useListedChannel()
  const now = useNow(60_000)
  if (!session) return null
  const showChannel = channel !== undefined && channel.id !== listed
  return (
    <div className="workspace-heading">
      <h2 ref={titleRef}>{session.title}</h2>
      <p>
        {showChannel ? (
          <>
            <button
              type="button"
              className="workspace-heading-channel"
              {...tooltip("Show in Sidebar")}
              onClick={() => {
                dispatch(revealSession({ sessionId }))
                frame.showRow(sessionId)
              }}
            >
              #{channel.name}
            </button>
            <span aria-hidden="true"> · </span>
          </>
        ) : null}
        {startedLabel(session.startedAt, now)}
      </p>
    </div>
  )
}
