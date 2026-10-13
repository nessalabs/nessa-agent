/**
 * The share dialog's commands (nessa_ui#128, #129), re-exported by
 * `commands.ts`, the one list. Each names the opening it acts in, so an
 * answer that comes after the dialog closed, or opened for another session,
 * changes nothing (`usecases/sharing.ts`, rows D1–D10 of the table in
 * `docs/design/read-grants.md`). An agent drives them as a person does:
 *
 * ```ts
 * store.dispatch(openShare({ sessionId }))
 * await store.dispatch(shareSession({ request: { credentialId, role: "read" } }))
 * await store.dispatch(revokeShare({ credentialId }))
 * ```
 */
import { sharingFailure } from "../../application/ports"
import { canChange } from "../../application/usecases/sharing"
import type { ShareChange } from "../../application/workspace-state"
import type { ShareDraft, ShareRequest } from "../../model/sharing"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const {
  shareOpened,
  sharesReading,
  sharesRead,
  sharesReadFailed,
  shareChanging,
  shareChanged,
  shareDrafted,
  shareRestarted,
} = workspaceActions

/** Read the open dialog's grants again, under `opening`. */
function readFor(opening: string, sessionId: string): WorkspaceCommand<Promise<void>> {
  return async (dispatch, _getState, { sharing, newId }) => {
    const read = newId()
    dispatch(sharesReading({ opening, read }))
    try {
      if (!sharing) throw new Error("No sharing source")
      dispatch(sharesRead({ opening, read, value: await sharing.read(sessionId) }))
    } catch (error) {
      dispatch(sharesReadFailed({ opening, read, reason: sharingFailure(error) }))
    }
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
    const read = newId()
    dispatch(shareOpened({ sessionId, opening, read }))
    try {
      dispatch(sharesRead({ opening, read, value: await sharing.read(sessionId) }))
    } catch (error) {
      dispatch(sharesReadFailed({ opening, read, reason: sharingFailure(error) }))
    }
  }
}

/** Set the open dialog's draft; held while a change is on its way (`shareDrafted`). */
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

/** Read the open dialog's grants again: its Try Again. */
export function readShares(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState) => {
    const dialog = getState().workspace.share
    if (dialog) await dispatch(readFor(dialog.opening, dialog.sessionId))
  }
}

/**
 * Carry out one change in the open dialog, then — if a dialog is open on the
 * same session by the time it settles, this one or one opened since — read
 * its grants again, whatever the outcome. Nothing begins while another
 * change is on its way, or before the dialog has read anything to change.
 */
function change(made: ShareChange): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { sharing, newId }) => {
    const dialog = getState().workspace.share
    if (!dialog || !sharing || !canChange(getState().workspace, dialog.opening)) return
    const { sessionId } = dialog
    dispatch(shareChanging({ opening: dialog.opening, change: made }))
    let outcome: Parameters<typeof shareChanged>[0]["outcome"]
    try {
      if (made.kind === "share") await sharing.share(sessionId, made.request, newId())
      else await sharing.unshare(sessionId, made.credentialId, newId())
      outcome = "done"
    } catch (error) {
      outcome = sharingFailure(error)
    }
    dispatch(shareChanged({ outcome }))
    const open = getState().workspace.share
    if (open?.sessionId === sessionId) await dispatch(readFor(open.opening, sessionId))
  }
}

/** Grant a device a role on the open dialog's session, or change what it holds. */
export function shareSession({
  request,
}: {
  request: ShareRequest
}): WorkspaceCommand<Promise<void>> {
  return change({ kind: "share", request })
}

/** Take away what a device holds on the open dialog's session. */
export function revokeShare({
  credentialId,
}: {
  credentialId: string
}): WorkspaceCommand<Promise<void>> {
  return change({ kind: "revoke", credentialId })
}
