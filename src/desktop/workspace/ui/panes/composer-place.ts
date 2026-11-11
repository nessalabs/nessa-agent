/**
 * Where a session's agent runs, in words, for the line under its composer:
 * the same words the conversation's heading uses (`placeNote`) — the lease
 * where there is one, else the channel's placement for this session — and
 * said always, "This computer" included, since the line is where a person
 * looks for it.
 */
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { useRememberedSession } from "../../adapters/store/remembered-session"
import { selectChannel } from "../../adapters/store/selectors"
import { entry } from "../../application/workspace-state"
import { placeNote } from "../transcript/transcript-heading"

export function useComposerPlace(sessionId: string): string | undefined {
  const session = useRememberedSession(sessionId)
  const channel = useWorkspaceSelector((state) =>
    session ? selectChannel(state, session.channelId) : undefined,
  )
  const lease = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.lease,
  )
  const placement =
    channel === undefined
      ? undefined
      : channel.coordinatorConversationId === sessionId
        ? channel.coordinatorPlacement
        : channel.threadPlacement
  return placeNote(lease, placement)
}
