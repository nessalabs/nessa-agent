/**
 * What follows from a change of state, whoever caused it — a click, a
 * keystroke, an agent's dispatch: a session on screen now — in a pane, or in
 * the open Agents overview (`shownSessionIds`) — has its conversation read,
 * once (a refused read waits for `retryTranscript`); the index read again —
 * the resync — reads every conversation on screen again, setting aside any
 * read already on its way, which may predate what the stream lost; a
 * session shown in a pane is marked read, here and at the source; and the
 * overview's filter is remembered.
 * Commands stay plain actions because these run beside the reducer rather
 * than inside each command.
 */
import {
  createListenerMiddleware,
  type ListenerMiddlewareInstance,
  type UnknownAction,
} from "@reduxjs/toolkit"
import { failureReason, type WorkspaceDependencies } from "../../application/ports"
import { shownSessionIds, unreadShown } from "../../application/usecases/updates"
import { entry, sessionOf, type WorkspaceState } from "../../application/workspace-state"
import { fromSource } from "../../model/revision"
import { workspaceActions } from "./slice"

type Root = { workspace: WorkspaceState }

export function workspaceEffects(
  dependencies: WorkspaceDependencies,
): ListenerMiddlewareInstance<Root> {
  const listener = createListenerMiddleware<Root>()
  // Reads in flight, so a session is asked for once however often panes
  // change. Each has its own token: a session let go while it was read — no
  // longer listed — forgets its read, and the answer, whenever it comes, is
  // let go with it; listed again and shown, it is read afresh. Each is
  // numbered in the order asked, so a resync can tell a read asked before
  // the index was (which may predate what the stream lost) from one after.
  const reading = new Map<string, { readonly token: symbol; readonly asked: number }>()
  let asked = 0
  // For each read of the index on its way, oldest first: the last read asked before it.
  const indexAsked: number[] = []

  /**
   * Reads a shown session's conversation, unless a read of it is on its way
   * already — asked after `since`, when given: an earlier one is set aside,
   * and its answer let go.
   */
  const read = (
    sessionId: string,
    dispatch: (action: UnknownAction) => unknown,
    since = -Infinity,
  ) => {
    const inFlight = reading.get(sessionId)
    if (inFlight && inFlight.asked > since) return
    const token = Symbol(sessionId)
    reading.set(sessionId, { token, asked: ++asked })
    const answered = (answer: () => void) => {
      if (reading.get(sessionId)?.token !== token) return
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
          answered(() => dispatch(workspaceActions.transcriptLoaded({ transcript }))),
        (error: unknown) =>
          answered(() =>
            dispatch(
              workspaceActions.transcriptFailed({
                sessionId,
                reason: failureReason(error),
              }),
            ),
          ),
      )
  }

  listener.startListening({
    predicate: (_action: UnknownAction, current, previous) =>
      current.workspace.panes !== previous.workspace.panes ||
      current.workspace.sessions !== previous.workspace.sessions ||
      current.workspace.content !== previous.workspace.content ||
      current.workspace.overview !== previous.workspace.overview ||
      current.workspace.transcriptFailures !== previous.workspace.transcriptFailures,
    effect: (_action, api) => {
      const state = api.getState().workspace
      for (const sessionId of [...reading.keys()])
        if (!sessionOf(state, sessionId)) reading.delete(sessionId)
      for (const sessionId of shownSessionIds(state)) {
        // Held, being read, or refused: a refused read waits to be asked for again.
        if (
          entry(state.transcripts, sessionId) ||
          entry(state.transcriptFailures, sessionId)
        )
          continue
        read(sessionId, api.dispatch)
      }
    },
  })

  // The index read again is the resync for what the stream lost: every
  // conversation on screen is read again, held or not, and a read already on
  // its way is set aside unless it was asked after the index was — one asked
  // before may predate the loss. The newer of the read's answer and what is
  // held is kept (`transcriptLoaded`).
  listener.startListening({
    actionCreator: workspaceActions.indexRequested,
    effect: () => {
      indexAsked.push(asked)
    },
  })
  listener.startListening({
    actionCreator: workspaceActions.indexFailed,
    effect: () => {
      indexAsked.shift()
    },
  })
  listener.startListening({
    actionCreator: workspaceActions.indexLoaded,
    effect: (_action, api) => {
      const since = indexAsked.shift() ?? asked
      for (const sessionId of shownSessionIds(api.getState().workspace))
        read(sessionId, api.dispatch, since)
    },
  })

  // The overview's filter is the store's; it is only kept outside, between launches.
  listener.startListening({
    predicate: (_action: UnknownAction, current, previous) =>
      current.workspace.overview.filter !== previous.workspace.overview.filter,
    effect: (_action, api) => {
      dependencies.overviewFilter.write(api.getState().workspace.overview.filter)
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
            console.warn("Could not mark a session read", sessionId, failureReason(error))
          })
      }
    },
  })

  return listener
}
