/**
 * What the person does to a session — send to it, answer its approval, choose
 * its model, archive it. The source owns sessions and their conversations, so
 * nothing here writes into what the source said: what the person does waits
 * beside it — a sent message in the outbox, an answer in `answers`, a chosen
 * model in `chosenModels` — until the source's own replacements settle it
 * (`updates.ts`). The one mark the window clears itself is "unread", since
 * showing a session is reading it. Pinning and archiving are the source's to
 * show; a session the source has not spoken of cannot be either yet.
 */
import { paneShowing } from "../../model/pane-layout"
import { knownToSource } from "../../model/revision"
import type { ModelRef } from "../../model/organisation"
import { startSession } from "../../model/session-lifecycle"
import { messageText, type Message } from "../../model/transcript"
import { failureText, type WorkspaceFailureReason } from "../ports"
import {
  draftOf,
  entry,
  forgetSession,
  modelForNextTurn,
  sessionOf,
  without,
  withSession,
  type WorkspaceState,
} from "../workspace-state"

/**
 * A session the source has not begun is the window's own: while a message
 * that starts it is on its way, the window shows it running.
 */
function startingRun(state: WorkspaceState, sessionId: string): WorkspaceState {
  const session = sessionOf(state, sessionId)
  return session && !knownToSource(session) && session.status !== "running"
    ? withSession(state, { ...session, status: "running" })
    : state
}

/**
 * The person sent a message. A draft starts with it — titled by it, listed
 * from now as running, at revision 0 until the source speaks of it — keeping
 * its id. The message waits in the outbox, marked as sending, until the
 * source's conversation includes it; what else a session the source has
 * begun shows is the source's to say.
 */
export function messageSent(
  state: WorkspaceState,
  { sessionId, message }: { sessionId: string; message: Message },
): WorkspaceState {
  const draft = draftOf(state, sessionId)
  if (!draft && !sessionOf(state, sessionId)) return state
  // The model chosen in the home is the person's, and stays theirs: no summary
  // the source sends of the new session speaks for it.
  const started = draft
    ? withSession(
        {
          ...state,
          drafts: without(state.drafts, sessionId),
          chosenModels: { ...state.chosenModels, [sessionId]: draft.model },
        },
        startSession(draft, messageText(message), message.at),
      )
    : startingRun(state, sessionId)
  return {
    ...started,
    outbox: {
      ...started.outbox,
      [sessionId]: [...(entry(started.outbox, sessionId) ?? []), message],
    },
  }
}

/** Changes one message in a session's outbox; the same state when nothing changes. */
function withSent(
  state: WorkspaceState,
  sessionId: string,
  messageId: string,
  change: (message: Message) => Message | null,
): WorkspaceState {
  const sent = entry(state.outbox, sessionId)
  const index = sent?.findIndex((message) => message.id === messageId) ?? -1
  if (!sent || index < 0) return state
  const changed = change(sent[index])
  if (changed === sent[index]) return state
  const rest = sent.flatMap((message, at) =>
    at !== index ? [message] : changed ? [changed] : [],
  )
  return {
    ...state,
    outbox:
      rest.length > 0
        ? { ...state.outbox, [sessionId]: rest }
        : without(state.outbox, sessionId),
  }
}

/** The source took the message; it is no longer marked as sending. */
export function messageDelivered(
  state: WorkspaceState,
  { sessionId, messageId }: { sessionId: string; messageId: string },
): WorkspaceState {
  return withSent(state, sessionId, messageId, (message) => {
    if (message.delivery?.state !== "sending") return message
    const { delivery: _delivered, ...delivered } = message
    return delivered
  })
}

/**
 * The source did not take the message: it stays, saying why. A session the
 * source has not begun is at rest once no message that could begin it is
 * left — none on its way, none taken; any other stands as the source last
 * said.
 */
export function sendFailed(
  state: WorkspaceState,
  {
    sessionId,
    messageId,
    reason,
  }: { sessionId: string; messageId: string; reason: WorkspaceFailureReason },
): WorkspaceState {
  const marked = withSent(state, sessionId, messageId, (message) => ({
    ...message,
    delivery: { state: "failed", reason: failureText(reason) },
  }))
  const session = sessionOf(marked, sessionId)
  return session &&
    !knownToSource(session) &&
    session.status === "running" &&
    !mayBegin(marked, sessionId)
    ? withSession(marked, { ...session, status: "idle" })
    : marked
}

/** A refused message is sent again, in its place and under its id. */
export function messageResent(
  state: WorkspaceState,
  { sessionId, messageId }: { sessionId: string; messageId: string },
): WorkspaceState {
  const resent = withSent(state, sessionId, messageId, (message) =>
    message.delivery?.state === "failed"
      ? { ...message, delivery: { state: "sending" } }
      : message,
  )
  return resent === state ? state : startingRun(resent, sessionId)
}

/**
 * Whether a message in the session's outbox may yet begin it at the source:
 * one on its way, or one the source took and has not yet shown.
 */
function mayBegin(state: WorkspaceState, sessionId: string): boolean {
  return (entry(state.outbox, sessionId) ?? []).some(
    (message) => message.delivery?.state !== "failed",
  )
}

/**
 * The person let a message the source refused go. A session the source never
 * began, left with nothing to begin it, was never a session: its pane goes
 * back to the new session's home it came from, under the same id, and it
 * leaves the lists.
 */
export function unsentDiscarded(
  state: WorkspaceState,
  { sessionId, messageId }: { sessionId: string; messageId: string },
): WorkspaceState {
  const discarded = withSent(state, sessionId, messageId, (message) =>
    message.delivery?.state === "failed" ? null : message,
  )
  const session = sessionOf(discarded, sessionId)
  if (
    discarded === state ||
    !session ||
    knownToSource(session) ||
    entry(discarded.outbox, sessionId)
  )
    return discarded
  const model = modelForNextTurn(discarded, sessionId) ?? session.model
  const unlisted = forgetSession(discarded, sessionId)
  return discarded.panes && paneShowing(discarded.panes, sessionId)
    ? {
        ...unlisted,
        drafts: {
          ...unlisted.drafts,
          [sessionId]: { id: sessionId, channelId: session.channelId, model },
        },
      }
    : unlisted
}

/**
 * The model the session's next turn runs on, chosen in its composer. Kept
 * beside the session, not in its summary, which is the source's: the next
 * message is sent with it.
 */
export function chooseModel(
  state: WorkspaceState,
  { sessionId, model }: { sessionId: string; model: ModelRef },
): WorkspaceState {
  const draft = draftOf(state, sessionId)
  if (draft)
    return { ...state, drafts: { ...state.drafts, [sessionId]: { ...draft, model } } }
  if (!sessionOf(state, sessionId)) return state
  return { ...state, chosenModels: { ...state.chosenModels, [sessionId]: model } }
}

/** An answer to an approval is on its way; it cannot be given twice meanwhile. */
export function approvalAnswering(
  state: WorkspaceState,
  {
    sessionId,
    approvalId,
    token,
  }: { sessionId: string; approvalId: string; token: string },
): WorkspaceState {
  return { ...state, answers: { ...state.answers, [sessionId]: { approvalId, token } } }
}

/**
 * The source did not confirm the answer: the approval asks again, saying why.
 * An answer that did reach the agent answers "already answered" if given
 * again, until the source's next conversation lets the approval go.
 */
export function approvalFailed(
  state: WorkspaceState,
  {
    sessionId,
    approvalId,
    token,
    reason,
  }: {
    sessionId: string
    approvalId: string
    token: string
    reason: WorkspaceFailureReason
  },
): WorkspaceState {
  // An earlier answer's refusal, arriving late, says nothing of the one on its way now.
  const given = entry(state.answers, sessionId)
  if (given?.approvalId !== approvalId || given.token !== token) return state
  return {
    ...state,
    answers: {
      ...state.answers,
      [sessionId]: { ...given, failure: failureText(reason) },
    },
  }
}

/**
 * The person has seen the session: its unread mark clears here at once, and
 * the source is told (`adapters/store/effects.ts`). An update that marks it
 * unread again while it is shown is read in turn.
 */
export function sessionRead(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  const session = sessionOf(state, sessionId)
  return session?.unread ? withSession(state, { ...session, unread: false }) : state
}
