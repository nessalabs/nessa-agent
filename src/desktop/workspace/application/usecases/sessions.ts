import type { FailureDiagnostic } from "../../model/failure"
/**
 * What the person does to a session — send to it, answer its approval, choose
 * its model, archive it. The source owns sessions and their conversations, so
 * nothing here writes into what the source said: what the person does waits
 * beside it — a sent message in the outbox, an answer in `answers`, a chosen
 * model in `chosenModels` or staged in `stagedModels` — until the source's own replacements settle it
 * (`updates.ts`). The one mark the window clears itself is "unread", since
 * showing a session is reading it. Pinning and archiving are the source's to
 * show; a session the source has not spoken of cannot be either yet.
 */
import type { Initiator } from "../ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import { knownToSource } from "../../model/revision"
import { sameModel, type ModelRef } from "../../model/workspace-index"
import { startSession } from "../../model/session-lifecycle"
import { messageText, type TurnMessage } from "../../model/transcript"
import { paneCount, removePane } from "../../../split-panes/model/pane-layout"
import {
  channelOf,
  draftOf,
  entry,
  forgetSession,
  modelForNextTurn,
  paneShowingSession,
  sessionOf,
  without,
  withPanes,
  withSession,
  type WorkspaceState,
} from "../workspace-state"
import { draftHome } from "./panes"

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
  {
    sessionId,
    message,
    initiator,
  }: { sessionId: string; message: TurnMessage; initiator: Initiator },
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
    // What the person typed goes when the person sent it — it is what was
    // sent. An agent's message is its own: what the person is typing stays.
    composerText:
      initiator === "person"
        ? without(started.composerText, sessionId)
        : started.composerText,
    outbox: {
      ...started.outbox,
      [sessionId]: [
        ...(entry(started.outbox, sessionId) ?? []),
        {
          ...message,
          observedInput: entry(state.transcripts, sessionId)?.latestInputId ?? null,
        },
      ],
    },
  }
}

/** Changes one message in a session's outbox; the same state when nothing changes. */
function withSent(
  state: WorkspaceState,
  sessionId: string,
  messageId: string,
  change: (message: TurnMessage) => TurnMessage | null,
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
    diagnostic,
    model,
  }: {
    sessionId: string
    messageId: string
    reason: WorkspaceFailureReason
    diagnostic?: FailureDiagnostic
    model?: ModelRef
  },
): WorkspaceState {
  const marked = withSent(state, sessionId, messageId, (message) => ({
    ...message,
    delivery: { state: "failed", reason, diagnostic, model },
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
      ? {
          ...message,
          observedInput: entry(state.transcripts, sessionId)?.latestInputId ?? null,
          delivery: { state: "sending" },
        }
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
 * back to a new session's home under the same id — in its channel while that
 * is still listed — and it leaves the lists. With no home to go back to, its
 * pane closes.
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
  const home = channelOf(discarded, session.channelId)
    ? { channelId: session.channelId, model }
    : draftHome(discarded, {})
  // The access chosen for it goes back with the home, as its model does.
  const access = entry(discarded.accessChoices, sessionId)
  const text = entry(discarded.composerText, sessionId)
  const unlisted = forgetSession(discarded, sessionId)
  if (!home) {
    const panes = unlisted.panes
    const showing = panes && paneShowingSession(panes, sessionId)
    if (!panes || !showing) return unlisted
    return withPanes(
      unlisted,
      paneCount(panes) > 1 ? removePane(panes, showing.key) : null,
    )
  }
  return discarded.panes && paneShowingSession(discarded.panes, sessionId)
    ? {
        ...unlisted,
        drafts: {
          ...unlisted.drafts,
          [sessionId]: { id: sessionId, ...home },
        },
        composerText:
          text === undefined
            ? unlisted.composerText
            : { ...unlisted.composerText, [sessionId]: text },
        accessChoices:
          access?.state === "chosen"
            ? { ...unlisted.accessChoices, [sessionId]: access }
            : unlisted.accessChoices,
      }
    : unlisted
}

/**
 * A model picked in a session's composer. A draft starts on it. An
 * established session only stages it: nothing is asked of the source until
 * the next message is sent (`model-commands.ts`). Picking the model the
 * session runs takes a switch back, since that send then asks nothing.
 * Never overrides the summary.
 */
export function chooseModel(
  state: WorkspaceState,
  { sessionId, model }: { sessionId: string; model: ModelRef },
): WorkspaceState {
  const draft = draftOf(state, sessionId)
  if (draft)
    return { ...state, drafts: { ...state.drafts, [sessionId]: { ...draft, model } } }
  const session = sessionOf(state, sessionId)
  if (!session || !knownToSource(session)) return state
  // A new pick is the person's newer word: an earlier one the source refused
  // is no longer said. One not known to be settled stays, so the next send
  // still checks it (`modelUnsettled`).
  const refused = entry(state.modelChoices, sessionId)?.state === "refused"
  // Even the model the summary shows stays staged: that model may still be
  // moving (a change of this window's or another client's can land late), so
  // only the send decides. A send whose pick the chat settled on asks nothing.
  return {
    ...state,
    stagedModels: { ...state.stagedModels, [sessionId]: model },
    modelChoices: refused ? without(state.modelChoices, sessionId) : state.modelChoices,
  }
}

/**
 * A staged pick the source made or refused outright: no longer waiting for
 * a send. Only that pick; a newer one made meanwhile still waits.
 */
export function stagedModelSettled(
  state: WorkspaceState,
  { sessionId, model }: { sessionId: string; model: ModelRef },
): WorkspaceState {
  const staged = entry(state.stagedModels, sessionId)
  return staged && sameModel(staged, model)
    ? { ...state, stagedModels: without(state.stagedModels, sessionId) }
    : state
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
  const others = (entry(state.answers, sessionId) ?? []).filter(
    (answer) => answer.approvalId !== approvalId,
  )
  return {
    ...state,
    answers: { ...state.answers, [sessionId]: [...others, { approvalId, token }] },
  }
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
  const answers = entry(state.answers, sessionId) ?? []
  const given = answers.find((answer) => answer.approvalId === approvalId)
  if (!given || given.token !== token) return state
  return {
    ...state,
    answers: {
      ...state.answers,
      [sessionId]: answers.map((answer) =>
        answer === given ? { ...given, failure: reason } : answer,
      ),
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

/**
 * What is typed in a session's composer and not sent — by the person, or an
 * agent writing it for them. The window's own, kept beside the session so it
 * outlives a change of layout or a pane showing something else; empty, it
 * is not kept. A session or new session the window does not hold keeps none.
 */
export function composerTextChanged(
  state: WorkspaceState,
  { sessionId, text }: { sessionId: string; text: string },
): WorkspaceState {
  if (!draftOf(state, sessionId) && !sessionOf(state, sessionId)) return state
  if (text === "") {
    const composerText = without(state.composerText, sessionId)
    return composerText === state.composerText ? state : { ...state, composerText }
  }
  if (entry(state.composerText, sessionId) === text) return state
  return { ...state, composerText: { ...state.composerText, [sessionId]: text } }
}
