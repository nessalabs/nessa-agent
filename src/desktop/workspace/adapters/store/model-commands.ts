import { knownToSource } from "../../model/revision"
/** Deliberate model choices; drafts keep their original creation snapshot. */
import {
  WorkspaceSourceError,
  WorkspaceModelError,
  failureReason,
} from "../../application/ports"
import {
  canChooseModel,
  entry,
  modelForNextTurn,
  sessionOf,
  type WorkspaceState,
} from "../../application/workspace-state"
import type { FailureDiagnostic, WorkspaceFailureReason } from "../../model/failure"
import { sameModel, type ModelRef } from "../../model/workspace-index"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

/**
 * Whether a model or access change is on its way, so a send waits: one this
 * window is asking, or one the source holds as pending, which no send could
 * settle (the source refuses a model request until it ends). A change this
 * window asked for that settled unconfirmed does not hold a send: the send
 * asks again first (`modelUnsettled`, `applyStagedModel`).
 */
export function modelChanging(state: WorkspaceState, sessionId: string): boolean {
  const choice = entry(state.modelChoices, sessionId)
  const source = entry(state.transcripts, sessionId)
  return (
    choice?.state === "asking" ||
    !!source?.modelControl?.pending ||
    !!source?.access?.changing
  )
}

/**
 * Whether a model change this window asked for settled unconfirmed. The
 * next send then asks for the model the picker shows before it goes, so no
 * message goes out on a model nobody confirmed.
 */
export function modelUnsettled(state: WorkspaceState, sessionId: string): boolean {
  const choice = entry(state.modelChoices, sessionId)
  return choice?.state === "acknowledged" || choice?.state === "recovery_required"
}

/** Only an active local call or access change holds another deliberate pick.
 * Recovery-required picks first reconcile in the source; Send stays fenced.
 */
export function modelRequesting(state: WorkspaceState, sessionId: string): boolean {
  const choice = entry(state.modelChoices, sessionId)
  const source = entry(state.transcripts, sessionId)
  return choice?.state === "asking" || !!source?.access?.changing
}

/**
 * A model picked in a session's composer: a draft starts on it; an
 * established session only stages it, and its next message applies it
 * (`applyStagedModel`). Picking its own model again takes the pick back.
 */
export function changeModel({
  sessionId,
  model,
}: {
  sessionId: string
  model: ModelRef
}): WorkspaceCommand {
  const target = { provider: model.provider, modelId: model.modelId }
  return (dispatch, getState) => {
    if (!canChooseModel(getState().workspace, sessionId)) return
    dispatch(workspaceActions.chooseModel({ sessionId, model: target }))
  }
}

/**
 * What applying a session's staged pick came to: the model its next message
 * is sent with, or why it is not sent.
 */
export type StagedModelOutcome =
  | { readonly applied: true; readonly model: ModelRef }
  | {
      readonly applied: false
      readonly reason: WorkspaceFailureReason
      readonly diagnostic?: FailureDiagnostic
    }

/**
 * Asks the source for the model staged in a session's composer, on the way
 * to sending its next message: the same-provider control or, for another
 * provider, the switch to its agent (`gateway-source.ts`). With nothing
 * staged, an unsettled model (`modelUnsettled`) is asked for again as shown,
 * which the source reconciles before it answers. Only a change the
 * source confirmed lets the message go, on the model it now runs, and the
 * pick is then made. One the source refused outright is dropped, so the
 * picker shows the model the session still runs; one not known either way
 * stays, so sending again checks it. With nothing staged and nothing
 * unsettled it asks nothing.
 */
export function applyStagedModel(
  sessionId: string,
): WorkspaceCommand<Promise<StagedModelOutcome>> {
  return async (dispatch, getState, { workspace, newId }) => {
    const state = getState().workspace
    const session = sessionOf(state, sessionId)
    const target = modelForNextTurn(state, sessionId)
    if (!session || !target) return { applied: false, reason: "unknown-session" }
    if (
      !knownToSource(session) ||
      (!entry(state.stagedModels, sessionId) && !modelUnsettled(state, sessionId))
    )
      return { applied: true, model: target }
    if (
      modelRequesting(state, sessionId) ||
      entry(state.accessChoices, sessionId)?.state === "changing"
    )
      return { applied: false, reason: "unavailable" }
    const requestId = newId()
    const choice = { requestId, model: target }
    dispatch(
      workspaceActions.modelChoiceAsked({
        sessionId,
        choice: { ...choice, state: "asking" },
      }),
    )
    try {
      const receipt = await workspace.setModel(sessionId, target, requestId, "person")
      if (receipt.requestId !== requestId || receipt.model !== target.modelId)
        throw new WorkspaceSourceError("unavailable")
      if (receipt.sourceReconciled) {
        dispatch(workspaceActions.modelChoiceCleared({ sessionId, requestId }))
        // Reconciled is a settled frame, not this target: another client may
        // have moved the chat on meanwhile. Only the model the source now
        // says it runs is sent on; another keeps the pick for the next send.
        const running = sessionOf(getState().workspace, sessionId)?.model
        if (!running || !sameModel(running, target))
          return { applied: false, reason: "unavailable" }
        dispatch(workspaceActions.stagedModelSettled({ sessionId, model: target }))
        return { applied: true, model: target }
      }
      dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId,
          choice: { ...choice, state: "acknowledged" },
        }),
      )
      return { applied: false, reason: "unavailable" }
    } catch (error) {
      const reason = failureReason(error)
      const refused = error instanceof WorkspaceModelError && !error.uncertain
      if (refused)
        dispatch(workspaceActions.stagedModelSettled({ sessionId, model: target }))
      const code =
        error instanceof WorkspaceSourceError ? error.diagnostic?.code : undefined
      dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId,
          choice: {
            ...choice,
            state: refused ? "refused" : "recovery_required",
            reason,
            ...(code ? { code } : {}),
          },
        }),
      )
      return {
        applied: false,
        reason,
        ...(error instanceof WorkspaceSourceError && error.diagnostic
          ? { diagnostic: error.diagnostic }
          : {}),
      }
    }
  }
}
