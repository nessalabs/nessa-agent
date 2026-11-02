import { conversationLeasePlace, conversationLeaseStatus } from "@nessa/client"
import type { RefObject } from "react"
import type { TranscriptLease } from "../../model/transcript"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"
import { useRememberedSession } from "../../adapters/store/remembered-session"
import { revealSession } from "../../adapters/store/commands"
import { useNow } from "../../../../clock"
import { startedLabel } from "../../model/time-labels"
import { useListedChannel, useWorkspaceFrame } from "../workspace-frame"
import { tooltip } from "../../../ui/tooltip"

/** Where a channel says its sessions run, until a lease says where one does. */
export type Placement = "local" | "boat"

/**
 * Where the agent runs, in words, always: this computer, Boat, an SSH host
 * or a paired machine. The lease is where it actually ran, so it decides;
 * before any lease, or one the gateway cannot read, the channel's placement
 * for this session says where it will.
 */
export function placeNote(
  lease: TranscriptLease | undefined,
  placement: Placement | undefined,
): string | undefined {
  if (lease && lease.state !== "unreadable" && lease.environment !== undefined)
    return conversationLeasePlace(lease)
  return placement === "boat"
    ? conversationLeasePlace({ environment: "boat" })
    : placement === "local"
      ? conversationLeasePlace({ environment: "here" })
      : undefined
}

/**
 * Where the agent's lease stands, in the client's words; nothing for a lease
 * the gateway cannot read, since the heading has nothing true to say of it.
 */
export function leaseNote(lease: TranscriptLease): string | undefined {
  if (lease.state === "unreadable") return undefined
  return conversationLeaseStatus(lease)
}

/**
 * A conversation's heading: its title, its channel, when it began, where its
 * agent runs and its lease, set lightly at the top of the transcript and
 * scrolling away with it. The
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
  const session = useRememberedSession(sessionId)
  const channel = useWorkspaceSelector((state) =>
    session ? selectChannel(state, session.channelId) : undefined,
  )
  const listed = useListedChannel()
  const now = useNow(60_000)
  if (!session) return null
  const showChannel = channel !== undefined && channel.id !== listed
  const placement =
    channel === undefined
      ? undefined
      : channel.coordinatorConversationId === sessionId
        ? channel.coordinatorPlacement
        : channel.threadPlacement
  const place = placeNote(lease, placement)
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
        {place ? (
          <>
            <span aria-hidden="true"> · </span>
            <span data-workspace-place={lease?.environment ?? placement}>{place}</span>
          </>
        ) : null}
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
