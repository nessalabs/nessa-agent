import { conversationLeasePlace, conversationLeaseStatus } from "@nessa/client"
import type { RefObject } from "react"
import type { TranscriptLease } from "../../model/transcript"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel } from "../../adapters/store/selectors"
import { useRememberedSession } from "../../adapters/store/remembered-session"
import { revealSession } from "../../adapters/store/commands"
import { useListedChannel, useWorkspaceFrame } from "../workspace-frame"
import { tooltip } from "../../../ui/tooltip"
/**
 * Where the agent ran, in words, when its lease says: every agent runs on
 * this gateway's own machine now (ADR 1417), so only a lease an older build
 * recorded elsewhere (an SSH host, a paired machine, a cloud VM) says
 * anything else. Nothing before any lease, or for one the gateway cannot
 * read.
 */
export function placeNote(lease: TranscriptLease | undefined): string | undefined {
  if (!lease || lease.state === "unreadable" || lease.environment === undefined)
    return undefined
  return conversationLeasePlace(lease)
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
  const where = lease?.environment
  const place = where === undefined || where === "here" ? undefined : placeNote(lease)
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
