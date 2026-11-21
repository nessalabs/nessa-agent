/**
 * The composer's access command: the shield's choice, carried to whoever owns
 * the session's access (`usecases/access.ts`). A session the source has not
 * begun holds it for its first message. One begun asks the source at once,
 * while the agent is at work too: the gateway changes a running turn's access,
 * so the very next tool call is decided at the new mode. Only when the source
 * cannot take it during that turn (a message queued behind it, say) is it held
 * as pending, and asked again once a conversation arrives with no turn at work
 * (`effects.ts`, `applyPendingAccess`). Any other refusal shows the source's
 * mode again, and says why.
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

const { accessChosen, accessChanging, accessQueued, accessTaken, accessRefused } =
  workspaceActions

/**
 * What became of a choice: `chosen` and held for the session's first
 * message; `pending` until the agent stops, when the running turn could not
 * take it; `taken` by the source; `refused`
 * by it, or not known to be taken (`unavailable`); `queued` behind the
 * change on its way — one change at a time, and this one is asked once that
 * settles; or `not-asked`, no such session.
 */
export type AccessOutcome =
  "chosen" | "pending" | "taken" | "refused" | "queued" | "not-asked"

/** Asks the source for `mode`, the choice shown as on its way until it answers. */
function askSource(
  sessionId: string,
  mode: AccessMode,
  initiator: Initiator,
  /**
   * Sent while a turn may be at work: a refusal the turn caused puts it back
   * as pending, to be asked again once that turn ends.
   */
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
    if (entry(state.accessChoices, sessionId)?.state === "changing") {
      dispatch(accessQueued({ sessionId, mode }))
      return "queued"
    }
    // Asked at once, at work or not: a person who raises access mid-turn
    // wants the next tool call decided at it, not after the turn has asked
    // them everything at the old one.
    return dispatch(
      askSource(sessionId, mode, initiator, atWork(entry(state.transcripts, sessionId))),
    )
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
