/**
 * What follows from a change of state, whoever caused it — a click, a
 * keystroke, an agent's dispatch: a session a pane now shows has its
 * conversation read, once (a refused read waits for `retryTranscript`), and
 * a session shown is marked read, here and at the source.
 * Commands stay plain actions because these run beside the reducer rather
 * than inside each command.
 */
import {
  createListenerMiddleware,
  type ListenerMiddlewareInstance,
  type UnknownAction,
} from "@reduxjs/toolkit"
import type { WorkspaceDependencies } from "../../application/ports"
import { shownSessionIds, unreadShown } from "../../application/usecases/updates"
import { entry, sessionOf, type WorkspaceState } from "../../application/workspace-state"
import { fromSource } from "../../model/revision"
import { refusal } from "./refusal"
import { workspaceActions } from "./slice"

type Root = { workspace: WorkspaceState }

export function workspaceEffects(
  dependencies: WorkspaceDependencies,
): ListenerMiddlewareInstance<Root> {
  const listener = createListenerMiddleware<Root>()
  // Reads in flight, so a session is asked for once however often panes
  // change. Each has its own token: a session let go while it was read — no
  // longer listed — forgets its read, and the answer, whenever it comes, is
  // let go with it; listed again and shown, it is read afresh.
  const reading = new Map<string, symbol>()

  listener.startListening({
    predicate: (_action: UnknownAction, current, previous) =>
      current.workspace.panes !== previous.workspace.panes ||
      current.workspace.sessions !== previous.workspace.sessions ||
      current.workspace.transcriptFailures !== previous.workspace.transcriptFailures,
    effect: (_action, api) => {
      const state = api.getState().workspace
      for (const sessionId of [...reading.keys()])
        if (!sessionOf(state, sessionId)) reading.delete(sessionId)
      for (const sessionId of shownSessionIds(state)) {
        // Held, being read, or refused: a refused read waits to be asked for again.
        if (
          entry(state.transcripts, sessionId) ||
          entry(state.transcriptFailures, sessionId) ||
          reading.has(sessionId)
        )
          continue
        const read = Symbol(sessionId)
        reading.set(sessionId, read)
        const answered = (answer: () => void) => {
          if (reading.get(sessionId) !== read) return
          reading.delete(sessionId)
          answer()
        }
        // Asked in a promise, so an adapter that throws still answers.
        Promise.resolve()
          .then(() => dependencies.workspace.transcript(sessionId))
          // An answer the window cannot use — another session's, or at a revision
          // the source could not have sent — is a fault, and the pane says so.
          .then((transcript) => {
            if (transcript.sessionId !== sessionId || !fromSource(transcript))
              throw new Error(
                `The source answered a read of ${sessionId} it could not have sent`,
              )
            return transcript
          })
          .then(
            (transcript) =>
              answered(() =>
                api.dispatch(workspaceActions.transcriptLoaded({ transcript })),
              ),
            (error: unknown) =>
              answered(() =>
                api.dispatch(
                  workspaceActions.transcriptFailed({
                    sessionId,
                    reason: refusal(error),
                  }),
                ),
              ),
          )
      }
    },
  })

  // Showing a session is reading it — the first one the workspace opens on,
  // one an agent opens, one marked unread again while shown. This is the one
  // place that says so, here and to the source.
  listener.startListening({
    predicate: (_action: UnknownAction, current, previous) =>
      current.workspace.panes !== previous.workspace.panes ||
      current.workspace.sessions !== previous.workspace.sessions,
    effect: (_action, api) => {
      for (const sessionId of unreadShown(api.getState().workspace)) {
        // Read already by a run this dispatch started: told once.
        if (!sessionOf(api.getState().workspace, sessionId)?.unread) continue
        api.dispatch(workspaceActions.sessionRead({ sessionId }))
        // Survivable: the mark stays cleared here, and the source says otherwise if it must.
        Promise.resolve()
          .then(() => dependencies.workspace.markRead(sessionId))
          .catch((error: unknown) => {
            console.warn("Could not mark a session read", sessionId, refusal(error))
          })
      }
    },
  })

  return listener
}
