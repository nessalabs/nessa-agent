import { conversationLeasePlace, conversationLeaseStatus } from "@nessa/client"
import type { RefObject } from "react"
import type { TranscriptLease } from "../../model/transcript"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"
import { useRememberedSession } from "../../adapters/store/remembered-session"
import { revealSession } from "../../adapters/store/commands"
import { useListedChannel, useWorkspaceFrame } from "../workspace-frame"
import { tooltip } from "../../../ui/tooltip"
import { channelPlaceKind, type ChannelPlace } from "../../model/channel-place"
import { channelPlaceWords } from "../channel-place-words"

/** Where a channel says its sessions run, until a lease says where one does. */
export type Placement = ChannelPlace

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
  if (placement === undefined) return undefined
  switch (channelPlaceKind(placement)) {
    case "boat":
      return conversationLeasePlace({ environment: "boat" })
    case "local":
      return conversationLeasePlace({ environment: "here" })
    default:
      return channelPlaceWords(placement)
  }
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
 * A conversation's heading: its title, set lightly at the top of the
 * transcript and scrolling away with it. A line under it says only what is
 * not the usual: its channel when the session list beside does not already
 * name it (chosen, it reveals the session in the sidebar), where its agent
 * runs when that is not this computer, and its lease when it is not simply
 * running. The shield and the pane menu say the rest, so a session on this
 * computer, running as it should, has no line at all.
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
  if (!session) return null
  const showChannel = channel !== undefined && channel.id !== listed
  const placement =
    channel === undefined
      ? undefined
      : channel.coordinatorConversationId === sessionId
        ? channel.coordinatorPlacement
        : channel.threadPlacement
  const where = lease?.environment ?? placement
  const place =
    where === "here" || where === "local" ? undefined : placeNote(lease, placement)
  const note = lease && lease.state !== "live" ? leaseNote(lease) : undefined
  const said = [
    showChannel ? (
      <button
        key="channel"
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
    ) : null,
    place ? (
      <span key="place" data-workspace-place={where}>
        {place}
      </span>
    ) : null,
    note ? <span key="note">{note}</span> : null,
  ].filter((each) => each !== null)
  return (
    <div className="workspace-heading">
      <h2 ref={titleRef}>{session.title}</h2>
      {said.length > 0 ? (
        <p>
          {said.flatMap((each, index) =>
            index === 0
              ? [each]
              : [
                  <span key={`dot-${index}`} aria-hidden="true">
                    {" · "}
                  </span>,
                  each,
                ],
          )}
        </p>
      ) : null}
    </div>
  )
}
