import type { RefObject } from "react"
import type { TranscriptLease } from "../../model/transcript"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel, selectSession } from "../../adapters/store/selectors"
import { revealSession } from "../../adapters/store/commands"
import { useNow } from "../../adapters/dom/clock"
import { startedLabel } from "../../model/time-labels"
import { useListedChannel, useWorkspaceFrame } from "../workspace-frame"
import { tooltip } from "../../../ui/tooltip"

/**
 * Where the agent runs, or why it does not, in a few words; nothing for a
 * lease the gateway cannot read.
 */
export function leaseNote(lease: TranscriptLease): string | undefined {
  switch (lease.state) {
    case "live":
      return lease.environment === "here" ? "On this computer" : undefined
    case "ending":
      return "Stopping"
    case "ended":
      switch (lease.cause) {
        case "closed":
          return "Closed"
        case "revoked":
          return "Access withdrawn"
        case "expired":
          return "Timed out"
        case "lost":
          return "Ended when Nessa restarted"
        case "stopped":
        case undefined:
          return "Stopped"
        default: {
          const exhaustive: never = lease.cause
          return exhaustive
        }
      }
    case "interrupted":
      return "Cleanup not confirmed"
    case "refused":
      return lease.refusal === "sandbox_unavailable"
        ? "Couldn't start: sandbox not available"
        : "Couldn't start"
    case "unreadable":
      return undefined
    default: {
      const exhaustive: never = lease.state
      return exhaustive
    }
  }
}

/**
 * A conversation's heading: its title, and where and when it began, set
 * lightly at the top of the transcript and scrolling away with it. The
 * channel is left out when the session list beside already names it, and
 * reveals the session in the sidebar when chosen.
 */
export function TranscriptHeading({
  sessionId,
  titleRef,
  lease,
}: {
  sessionId: string
  titleRef: RefObject<HTMLHeadingElement | null>
  lease?: TranscriptLease
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
  const note = lease && leaseNote(lease)
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
        {note ? (
          <>
            <span aria-hidden="true"> · </span>
            <span>{note}</span>
          </>
        ) : null}
      </p>
    </div>
  )
}
