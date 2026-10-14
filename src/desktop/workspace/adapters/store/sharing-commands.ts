/**
 * The share dialog's commands (nessa_ui#128, #129), re-exported by
 * `commands.ts`, the one list. Each names the opening it acts in, so an
 * answer that comes after the dialog closed, or opened for another session,
 * changes nothing (`usecases/sharing.ts`, rows D1–D11 of the table in
 * `docs/design/read-grants.md`). An agent drives them as a person does:
 * it sets the draft, and Share sends the draft as the store holds it then —
 * the draft is the only source of what is sent, so nothing an agent or a
 * stale render passes can differ from the basis the draft builds on.
 *
 * ```ts
 * await store.dispatch(openShare({ sessionId }))
 * store.dispatch(draftShare({ draft: { ...emptyDraft(credentialId), role: "read" } }))
 * await store.dispatch(shareSession())
 * store.dispatch(askRevoke({ credentialId }))
 * await store.dispatch(revokeShare())
 * ```
 */
import { sharingFailure, type SharingSource } from "../../application/ports"
import {
  canChange,
  type ChangeSettled,
  type ReadAnswer,
} from "../../application/usecases/sharing"
import type { ShareChange } from "../../application/workspace-state"
import { requestFrom, type ShareDraft } from "../../model/sharing"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const {
  shareOpened,
  sharesReading,
  sharesRead,
  shareChanging,
  shareChanged,
  shareDrafted,
  shareRestarted,
  shareConfirming,
  shareConfirmCancelled,
} = workspaceActions

/** One part of a read, settled either way. */
async function answer<T>(call: () => Promise<T>): Promise<ReadAnswer<T>> {
  try {
    return { ok: true, value: await call() }
  } catch (error) {
    return { ok: false, reason: sharingFailure(error) }
  }
}

/**
 * Ask read `read` of the open dialog `opening`: its grants and what its
 * form offers, apart, so either can fail alone (D4). Only the newest read
 * the dialog names is applied (D2).
 */
function readInto(
  sharing: SharingSource,
  opening: string,
  sessionId: string,
  read: string,
): WorkspaceCommand<Promise<void>> {
  return async (dispatch) => {
    const [grants, choices] = await Promise.all([
      answer(() => sharing.grants(sessionId)),
      answer(() => sharing.choices()),
    ])
    dispatch(sharesRead({ opening, read, grants, choices }))
  }
}

/**
 * Open the share dialog for a session, and read who it is shared with. A
 * window with no sharing source opens nothing.
 */
export function openShare({
  sessionId,
}: {
  sessionId: string
}): WorkspaceCommand<Promise<void>> {
  return async (dispatch, _getState, { sharing, newId }) => {
    if (!sharing) return
    const opening = newId()
    const asked = newId()
    dispatch(shareOpened({ sessionId, opening, read: asked }))
    await dispatch(readInto(sharing, opening, sessionId, asked))
  }
}

/** Set the open dialog's draft; not taken while the form is held (`formHeld`, D10). */
export function draftShare({ draft }: { draft: ShareDraft }): WorkspaceCommand<void> {
  return (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (dialog) dispatch(shareDrafted({ opening: dialog.opening, draft }))
  }
}

/** Start the open dialog's draft again from what its device holds now: its Start Again. */
export function restartShare(): WorkspaceCommand<void> {
  return (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (dialog) dispatch(shareRestarted({ opening: dialog.opening }))
  }
}

/** Read the open dialog's grants, and what its form offers, again: its Try Again. */
export function readShares(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { sharing, newId }) => {
    const dialog = getState().workspace.share
    if (!dialog || !sharing) return
    const asked = newId()
    dispatch(sharesReading({ opening: dialog.opening, read: asked }))
    await dispatch(readInto(sharing, dialog.opening, dialog.sessionId, asked))
  }
}

/**
 * Carry out one change in the open dialog, then — if a dialog is open on the
 * same session by the time it settles, this one or one opened since — read
 * its grants again, whatever the outcome. Nothing begins while another
 * change is on its way, or before the dialog has read its grants. What the
 * gateway answered a made change with is what the draft builds on (D10).
 */
function change(made: ShareChange): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { sharing, newId }) => {
    const dialog = getState().workspace.share
    if (!dialog || !sharing || !canChange(getState().workspace, dialog.opening)) return
    const { sessionId } = dialog
    dispatch(shareChanging({ opening: dialog.opening, change: made }))
    let settled: ChangeSettled
    try {
      if (made.kind === "share")
        settled = {
          outcome: "done",
          held: await sharing.share(sessionId, made.request, newId()),
        }
      else {
        await sharing.unshare(
          sessionId,
          made.credentialId,
          made.expectedPolicyRevision,
          newId(),
        )
        settled = { outcome: "done", held: null }
      }
    } catch (error) {
      settled = { outcome: sharingFailure(error) }
    }
    const asked = newId()
    dispatch(shareChanged({ settled, read: asked }))
    const open = getState().workspace.share
    if (open?.sessionId === sessionId && open.reading === asked)
      await dispatch(readInto(sharing, open.opening, sessionId, asked))
  }
}

/**
 * Share the open dialog's draft: grant its device its role, or change what
 * it holds. What is sent is built from the draft as the store holds it now
 * (`requestFrom`), the one source of it, so the basis the answer moves the
 * draft onto is the basis of what was sent. A draft that cannot be sent —
 * no device, changed under it, past a bound — or a list or offer not read
 * sends nothing.
 */
export function shareSession(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (!dialog?.grants || !dialog.choices) return
    const outcome = requestFrom(dialog.draft, dialog.grants, dialog.choices.limits)
    if (outcome.ok) await dispatch(change({ kind: "share", request: outcome.request }))
  }
}

/** Ask to revoke a device's grant on the open dialog's session: Revoke, before it is confirmed. */
export function askRevoke({
  credentialId,
}: {
  credentialId: string
}): WorkspaceCommand<void> {
  return (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (dialog) dispatch(shareConfirming({ opening: dialog.opening, credentialId }))
  }
}

/** Say no to the Revoke being asked. */
export function cancelRevoke(): WorkspaceCommand<void> {
  return (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (dialog) dispatch(shareConfirmCancelled({ opening: dialog.opening }))
  }
}

/**
 * Take away what the device being asked about holds, naming the revision it
 * was listed at when asked: refused `changed` if it has moved since. With no
 * Revoke being asked — none was, or a change or a read ended it — nothing.
 */
export function revokeShare(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState) => {
    const asked = getState().workspace.share?.confirming
    if (!asked) return
    await dispatch(
      change({
        kind: "revoke",
        credentialId: asked.credentialId,
        expectedPolicyRevision: asked.policyRevision,
      }),
    )
  }
}
