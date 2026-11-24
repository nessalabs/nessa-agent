/**
 * The person's choice of access for a session (`model/access.ts`). A session
 * the source has not begun holds it until its first message carries it; one
 * begun asks the source, and the choice waits beside the conversation until
 * the conversation says it (`withTranscript`), as an answer to an approval
 * does. A refusal leaves the source's mode showing, and says why.
 */
import { knownToSource } from "../../model/revision"
import { atWork } from "../../model/transcript"
import type { WorkspaceFailureReason } from "../../model/failure"
import {
  settledBy,
  shownAccess,
  type AccessChoice,
  type AccessMode,
  type SessionAccess,
} from "../../model/access"
import {
  draftOf,
  entry,
  sessionOf,
  without,
  type WorkspaceState,
} from "../workspace-state"

type Change = { sessionId: string; mode: AccessMode }

/** Whether the source has begun the session, so a change is asked of it rather than held. */
export function accessAskedOfSource(state: WorkspaceState, sessionId: string): boolean {
  if (draftOf(state, sessionId)) return false
  const session = sessionOf(state, sessionId)
  return session !== undefined && knownToSource(session)
}

/** The session's access as the shield shows it, with what the source said and the choice beside it. */
export function accessOf(
  state: WorkspaceState,
  sessionId: string,
): {
  readonly mode: AccessMode
  readonly source: SessionAccess | undefined
  readonly choice: AccessChoice | undefined
} {
  const source = entry(state.transcripts, sessionId)?.access
  const choice = entry(state.accessChoices, sessionId)
  return { mode: shownAccess(source, choice), source, choice }
}

/** The access a session's first message begins it at: one chosen before it began. */
export function accessToStart(
  state: WorkspaceState,
  sessionId: string,
): AccessMode | undefined {
  const choice = entry(state.accessChoices, sessionId)
  return choice?.state === "chosen" ? choice.mode : undefined
}

function withChoice(
  state: WorkspaceState,
  sessionId: string,
  choice: AccessChoice | undefined,
): WorkspaceState {
  const held = entry(state.accessChoices, sessionId)
  if (held === choice) return state
  return {
    ...state,
    accessChoices: choice
      ? { ...state.accessChoices, [sessionId]: choice }
      : without(state.accessChoices, sessionId),
    // A choice the person makes answers a step a model change made.
    steppedAccess:
      choice?.state === "chosen" || choice?.state === "changing"
        ? without(state.steppedAccess, sessionId)
        : state.steppedAccess,
  }
}

/** Chosen for a session the source has not begun: its first message carries it. */
export function accessChosen(
  state: WorkspaceState,
  { sessionId, mode }: Change,
): WorkspaceState {
  if (accessAskedOfSource(state, sessionId)) return state
  if (!draftOf(state, sessionId) && !sessionOf(state, sessionId)) return state
  return withChoice(state, sessionId, { mode, state: "chosen" })
}

/**
 * The pending choices to ask of the source now: one a new session's
 * conversation made pending (a pick after its first message) goes at once,
 * mid-turn too; one a running turn could not take goes once a conversation
 * newer than that refusal has no turn at work. The one owner of when a
 * pending choice goes — `effects.ts` asks after every change of state.
 */
export function pendingAccess(
  state: WorkspaceState,
): readonly { readonly sessionId: string; readonly mode: AccessMode }[] {
  return Object.keys(state.accessChoices).flatMap((sessionId) => {
    const choice = entry(state.accessChoices, sessionId)
    const transcript = entry(state.transcripts, sessionId)
    return choice?.state === "pending" &&
      transcript &&
      (choice.after === undefined ||
        (!atWork(transcript) && transcript.revision > choice.after))
      ? [{ sessionId, mode: choice.mode }]
      : []
  })
}

/**
 * A first message carries the chosen access to the source's create: that
 * choice is captured. A pick after this one is not, and stays the newer word.
 */
export function accessCaptured(
  state: WorkspaceState,
  { sessionId, mode }: Change,
): WorkspaceState {
  const held = entry(state.accessChoices, sessionId)
  if (held?.state !== "chosen" || held.mode !== mode || held.captured) return state
  return withChoice(state, sessionId, { mode, state: "chosen", captured: true })
}

/** Asked of the source; shown as chosen while it is on its way. */
export function accessChanging(
  state: WorkspaceState,
  { sessionId, mode }: Change,
): WorkspaceState {
  if (!accessAskedOfSource(state, sessionId)) return state
  return withChoice(state, sessionId, { mode, state: "changing" })
}

/**
 * Picked while a change is on its way: held beside it as the next to ask,
 * so the latest pick stands. Picking the one on its way again drops the
 * next. Nothing on its way, nothing is held.
 */
export function accessQueued(
  state: WorkspaceState,
  { sessionId, mode }: Change,
): WorkspaceState {
  const held = entry(state.accessChoices, sessionId)
  if (held?.state !== "changing") return state
  return withChoice(
    state,
    sessionId,
    held.mode === mode
      ? { mode, state: "changing" }
      : { mode: held.mode, state: "changing", next: mode },
  )
}

/**
 * The source committed it. Let go at once when the conversation held says it
 * already — its update arrived before the call resolved — and otherwise kept
 * until one does. Only the change on its way settles: a later choice is newer.
 */
export function accessTaken(
  state: WorkspaceState,
  { sessionId, mode }: Change,
): WorkspaceState {
  const held = entry(state.accessChoices, sessionId)
  if (held?.state !== "changing" || held.mode !== mode) return state
  // A pick made meanwhile is the newer word: it goes next, at once.
  if (held.next !== undefined)
    return withChoice(state, sessionId, { mode: held.next, state: "pending" })
  const taken: AccessChoice = { mode, state: "taken" }
  const transcript = entry(state.transcripts, sessionId)
  return withChoice(state, sessionId, transcript ? settledBy(taken, transcript) : taken)
}

/** The source did not take it: its own mode shows again, and the reason is kept to say. */
export function accessRefused(
  state: WorkspaceState,
  {
    sessionId,
    mode,
    reason,
    code,
    requeue,
  }: Change & { reason: WorkspaceFailureReason; code?: string; requeue?: boolean },
): WorkspaceState {
  const held = entry(state.accessChoices, sessionId)
  if (held?.state !== "changing" || held.mode !== mode) return state
  // A pick made meanwhile is the newer word, refusal or not: it goes next.
  if (held.next !== undefined)
    return withChoice(state, sessionId, { mode: held.next, state: "pending" })
  // A choice the running turn could not take is still the person's: a turn
  // that began first (`turn_running`), or one the agent could not change
  // under while it was at work (`approval_mode_not_applied`, such as a
  // message queued behind the turn). It waits for a conversation newer than
  // the one held, so it is asked again only once that turn ends.
  const transcript = entry(state.transcripts, sessionId)
  const turnCaused =
    code === "turn_running" ||
    (code === "approval_mode_not_applied" &&
      transcript !== undefined &&
      atWork(transcript))
  if (requeue && turnCaused)
    return withChoice(state, sessionId, {
      mode,
      state: "pending",
      after: transcript?.revision ?? 0,
    })
  return withChoice(state, sessionId, {
    mode,
    state: "refused",
    reason,
    ...(code === undefined ? {} : { code }),
  })
}
