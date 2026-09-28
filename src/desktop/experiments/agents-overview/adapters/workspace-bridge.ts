/**
 * Everything the agents overview reads from the workspace, or asks of its
 * source, in one file: narrow selectors over the workspace's state, and two
 * thunks that reach the source through the store's own dependencies.
 *
 * The thunks stand in for workspace API this experiment needs and the
 * workspace does not offer yet:
 *
 * - **Reading a conversation no pane shows** (`readConversation`). The
 *   window keeps the conversations its panes show and a few others
 *   (`workspace/model/retention.ts`); the overview needs the approval of every
 *   waiting session, and a peek at whichever session is chosen, shown in a
 *   pane or not. It reads them itself and keeps nothing in the store; what
 *   the window holds, kept current by the source's stream, wins when newer.
 * - **Answering an approval the overview shows** (`answerRequest`). The
 *   workspace's `approve` and `deny` answer only an approval a pane shows
 *   (ADR 238, "only an approval on screen is answered"); the overview puts
 *   approvals on screen without a pane. It asks the source directly, as the
 *   person, and the source records the decision and who made it as it does
 *   for the pane's card. The workspace follows from the source's updates.
 *
 * Once the workspace offers both, these two go and the overview dispatches
 * the workspace's own commands.
 */
import { createSelector } from "@reduxjs/toolkit"
import type { DesktopState } from "../../../store"
import type { WorkspaceCommand } from "../../../workspace"
import { failureReason } from "../../../workspace/application/ports"
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import type { SessionSummary } from "../../../workspace/model/overview"
import { focusedPane } from "../../../workspace/model/pane-layout"
import { agentsGlance, type AgentsGlance, type Held } from "../model/agents-glance"
import type { Transcript } from "../../../workspace/model/transcript"
import type { Answer } from "../model/request"

type Root = Pick<DesktopState, "workspace">

/** A record keyed by an id from outside this module, read for what it holds. */
function own<T>(record: Readonly<Record<string, T>>, key: string): T | undefined {
  return Object.hasOwn(record, key) ? record[key] : undefined
}

const listed = createSelector(
  [(state: Root) => state.workspace.sessions],
  (sessions): readonly SessionSummary[] => Object.values(sessions),
)

/** The overview's groups; compare with `sameGlance`. */
export const selectGlance = (state: Root, held: readonly Held[]): AgentsGlance =>
  agentsGlance(listed(state), held)

export const selectSummary = (state: Root, sessionId: string) =>
  own(state.workspace.sessions, sessionId)

/** The conversation the window holds for a session, if it holds one. */
export const selectHeldConversation = (
  state: Root,
  sessionId: string,
): Transcript | undefined => own(state.workspace.transcripts, sessionId)

export const selectChannelName = (state: Root, channelId: string) =>
  state.workspace.channels.find((channel) => channel.id === channelId)?.name

/** The session in the focused pane, so opening one elsewhere can put the overview away. */
export const selectFocusedSessionId = (state: Root): string | null =>
  state.workspace.panes ? focusedPane(state.workspace.panes).sessionId : null

/** Whether the workspace has read its overview and can list sessions. */
export const selectReady = (state: Root) => state.workspace.status === "ready"

/** What a read of a session's conversation came to. */
export type ConversationRead =
  | { readonly kind: "read"; readonly transcript: Transcript }
  | { readonly kind: "failed"; readonly reason: WorkspaceFailureReason }

/** Reads a session's conversation — what it did, what it asks — from the source. */
export function readConversation(
  sessionId: string,
): WorkspaceCommand<Promise<ConversationRead>> {
  return async (_dispatch, _getState, { workspace }) => {
    try {
      const transcript = await workspace.transcript(sessionId)
      if (transcript.sessionId !== sessionId)
        throw new Error(`The source answered a read of ${sessionId} with another session`)
      return { kind: "read", transcript }
    } catch (error) {
      return { kind: "failed", reason: failureReason(error) }
    }
  }
}

/** What became of an answer: taken, or not, with the source's reason. */
export type AnswerSent =
  | { readonly kind: "sent" }
  | { readonly kind: "failed"; readonly reason: WorkspaceFailureReason }

/**
 * Answers the approval the overview shows, as the person. The source refuses
 * an approval it no longer asks (`not-waiting`), so an answer given twice —
 * here and in a pane — is taken once.
 */
export function answerRequest({
  sessionId,
  approvalId,
  answer,
}: {
  sessionId: string
  approvalId: string
  answer: Answer
}): WorkspaceCommand<Promise<AnswerSent>> {
  return async (_dispatch, _getState, { workspace }) => {
    try {
      if (answer === "deny") await workspace.deny(sessionId, approvalId, "person")
      else
        await workspace.approve(
          sessionId,
          approvalId,
          answer === "always" ? "always" : "once",
          "person",
        )
      return { kind: "sent" }
    } catch (error) {
      return { kind: "failed", reason: failureReason(error) }
    }
  }
}
