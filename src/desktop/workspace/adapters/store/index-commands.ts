/**
 * The index commands: read the workspace's index and follow the source.
 * Re-exported by `commands.ts`, the one list.
 */
import {
  failureReason,
  failureStages,
  type WorkspaceUpdate,
} from "../../application/ports"
import { consistentIndex, contradicts } from "../../model/workspace-index"
import { fromSource } from "../../model/revision"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const { indexFailed, indexLoaded, indexRequested, sessionRemoved, updateReceived } =
  workspaceActions

/**
 * Reads the index and opens the workspace on it. Open already, it is the
 * resync for whatever the stream lost: a session the index no longer lists
 * is taken out, and the conversations the panes show are read again.
 */
export function loadWorkspace(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, _getState, { workspace, newId }) => {
    const read = newId()
    dispatch(indexRequested({ read }))
    try {
      const index = await workspace.index()
      const unusable = index.sessions.filter((session) => !fromSource(session))
      // Let go by the reducer; said here, where the source's answer is still at hand.
      if (unusable.length > 0)
        console.warn(
          "The index listed sessions at a revision the source could not have sent",
          unusable.map((session) => session.id),
        )
      // Left out by the reducer (`consistentIndex`); said here, by id.
      const { contradictions } = consistentIndex(index)
      if (contradicts(contradictions))
        console.warn("The index contradicted itself; left out, by id:", contradictions)
      dispatch(indexLoaded({ index, draftId: newId(), read }))
    } catch (error) {
      dispatch(
        indexFailed({
          reason: failureReason(error),
          stages: failureStages(error),
          read,
        }),
      )
    }
  }
}

/** The session an update is of, whichever it is. */
function sessionOfUpdate(update: Exclude<WorkspaceUpdate, { kind: "resync" }>): string {
  switch (update.kind) {
    case "session":
      return update.session.id
    case "session-removed":
      return update.sessionId
    case "transcript":
      return update.transcript.sessionId
  }
}

/** The revision an update carries, whichever it is. */
function revisionOf(update: Exclude<WorkspaceUpdate, { kind: "resync" }>): number {
  switch (update.kind) {
    case "session":
      return update.session.revision
    case "session-removed":
      return update.revision
    case "transcript":
      return update.transcript.revision
  }
}

/**
 * Follows the source's changes until the returned function is called. An
 * update at a revision the source could not have sent is let go, and said.
 * A `resync` — the source may have lost updates — reads the index again
 * (`loadWorkspace`), and with it every conversation on screen.
 */
export function followWorkspace(): WorkspaceCommand<() => void> {
  return (dispatch, _getState, { workspace, newId }) =>
    workspace.subscribe((update) => {
      if (update.kind === "resync") {
        void dispatch(loadWorkspace())
        return
      }
      if (!fromSource({ revision: revisionOf(update) })) {
        // Said by what it is and whose, never with what it holds: a transcript
        // carries the person's words, code and commands.
        console.warn("The workspace source sent an update it could not have sent", {
          kind: update.kind,
          sessionId: sessionOfUpdate(update),
          revision: revisionOf(update),
        })
        return
      }
      dispatch(
        update.kind === "session-removed"
          ? // A removal may empty the last pane, which then starts over.
            sessionRemoved({
              sessionId: update.sessionId,
              revision: update.revision,
              draftId: newId(),
            })
          : updateReceived({ update }),
      )
    })
}
