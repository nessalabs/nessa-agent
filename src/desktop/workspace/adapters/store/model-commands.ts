import { knownToSource } from "../../model/revision"
/** Deliberate model choices; drafts keep their original creation snapshot. */
import {
  WorkspaceSourceError,
  WorkspaceModelError,
  failureReason,
} from "../../application/ports"
import {
  draftOf,
  entry,
  sessionOf,
  type WorkspaceState,
} from "../../application/workspace-state"
import type { ModelRef } from "../../model/workspace-index"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

export function modelChanging(state: WorkspaceState, sessionId: string): boolean {
  const choice = entry(state.modelChoices, sessionId)
  const source = entry(state.transcripts, sessionId)
  return (
    (!!choice && choice.state !== "refused") ||
    !!source?.modelControl?.pending ||
    !!source?.access?.changing
  )
}

/** Only an active call or live changing frame holds another deliberate pick.
 * Recovery-required picks first reconcile in the source; Send stays fenced.
 */
export function modelRequesting(state: WorkspaceState, sessionId: string): boolean {
  const choice = entry(state.modelChoices, sessionId)
  const source = entry(state.transcripts, sessionId)
  return (
    choice?.state === "asking" ||
    source?.modelControl?.pending?.status === "changing" ||
    !!source?.access?.changing
  )
}

export function changeModel({
  sessionId,
  model,
}: {
  sessionId: string
  model: ModelRef
}): WorkspaceCommand<Promise<void>> {
  const target = { provider: model.provider, modelId: model.modelId }
  return async (dispatch, getState, { workspace, newId }) => {
    const state = getState().workspace
    if (draftOf(state, sessionId)) {
      dispatch(workspaceActions.chooseModel({ sessionId, model: target }))
      return
    }
    const session = sessionOf(state, sessionId)
    if (
      !session ||
      !knownToSource(session) ||
      session.model.provider !== target.provider ||
      modelRequesting(state, sessionId) ||
      entry(state.accessChoices, sessionId)?.state === "changing"
    )
      return
    if (session.model.modelId === target.modelId && !modelChanging(state, sessionId))
      return
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
      if (receipt.sourceReconciled)
        dispatch(workspaceActions.modelChoiceCleared({ sessionId, requestId }))
      else
        dispatch(
          workspaceActions.modelChoiceSettled({
            sessionId,
            choice: { ...choice, state: "acknowledged" },
          }),
        )
    } catch (error) {
      dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId,
          choice: {
            ...choice,
            state:
              error instanceof WorkspaceModelError && !error.uncertain
                ? "refused"
                : "recovery_required",
            reason: failureReason(error),
            ...(error instanceof WorkspaceSourceError && error.diagnostic?.code
              ? { code: error.diagnostic.code }
              : {}),
          },
        }),
      )
    }
  }
}
