/**
 * The composer's access command: the shield's choice, carried to whoever owns
 * the session's access (`usecases/access.ts`). A session the source has not
 * begun holds it for its first message. One begun asks the source, which
 * commits it for every review decided after it — but only between turns: a
 * choice made while the agent is at work is held as pending, and asked of the
 * source once a conversation arrives with no turn at work (`effects.ts`,
 * `applyPendingAccess`). A refusal shows the source's mode again, and says why.
 *
 * ```ts
 * await store.dispatch(chooseAccess({ sessionId, mode: "full", initiator: "person" }))
 * ```
 */
import {
  failureReason,
  WorkspaceSourceError,
  type Initiator,
} from "../../application/ports"
import { accessAskedOfSource } from "../../application/usecases/access"
import type { AccessMode } from "../../model/access"
import { atWork } from "../../model/transcript"
import { entry } from "../../application/workspace-state"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const { accessChosen, accessPending, accessChanging, accessTaken, accessRefused } =
  workspaceActions

/**
 * What became of a choice: `chosen` and held for the session's first
 * message; `pending` until the agent stops; `taken` by the source; `refused`
 * by it, or not known to be taken (`unavailable`); `changing` already — one
 * change at a time, and the one on its way settles first; or `not-asked`, no
 * such session.
 */
export type AccessOutcome =
  "chosen" | "pending" | "taken" | "refused" | "changing" | "not-asked"

/** Asks the source for `mode`, the choice shown as on its way until it answers. */
function askSource(
  sessionId: string,
  mode: AccessMode,
  initiator: Initiator,
  /** Sent from pending: a refusal because a turn began first puts it back. */
  requeue = false,
): WorkspaceCommand<Promise<AccessOutcome>> {
  return async (dispatch, getState, { workspace }) => {
    dispatch(accessChanging({ sessionId, mode }))
    try {
      await workspace.setAccess(sessionId, mode, initiator)
      dispatch(accessTaken({ sessionId, mode }))
      return "taken"
    } catch (error) {
      const code =
        error instanceof WorkspaceSourceError ? error.diagnostic?.code : undefined
      dispatch(
        accessRefused({
          sessionId,
          mode,
          reason: failureReason(error),
          ...(code === undefined ? {} : { code }),
          requeue,
        }),
      )
      return entry(getState().workspace.accessChoices, sessionId)?.state === "pending"
        ? "pending"
        : "refused"
    }
  }
}

export function chooseAccess({
  sessionId,
  mode,
  initiator,
}: {
  sessionId: string
  mode: AccessMode
  initiator: Initiator
}): WorkspaceCommand<Promise<AccessOutcome>> {
  return async (dispatch, getState) => {
    const state = getState().workspace
    if (!accessAskedOfSource(state, sessionId)) {
      dispatch(accessChosen({ sessionId, mode }))
      return entry(getState().workspace.accessChoices, sessionId)?.mode === mode
        ? "chosen"
        : "not-asked"
    }
    if (entry(state.accessChoices, sessionId)?.state === "changing") return "changing"
    if (atWork(entry(state.transcripts, sessionId))) {
      dispatch(accessPending({ sessionId, mode }))
      return "pending"
    }
    return dispatch(askSource(sessionId, mode, initiator))
  }
}

/**
 * Asks the source for a pending choice, now that its conversation has no
 * turn at work. The person chose it, so it goes as theirs. A choice no longer
 * pending — replaced, or let go — is not sent.
 */
export function applyPendingAccess({
  sessionId,
  mode,
}: {
  sessionId: string
  mode: AccessMode
}): WorkspaceCommand<Promise<AccessOutcome>> {
  return async (dispatch, getState) => {
    const held = entry(getState().workspace.accessChoices, sessionId)
    if (held?.state !== "pending" || held.mode !== mode) return "not-asked"
    return dispatch(askSource(sessionId, mode, "person", true))
  }
}
