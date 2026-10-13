/**
 * The share dialog's state, as pure transitions over the workspace
 * (nessa_ui#128, #129). The orderings are the table in
 * `docs/design/read-grants.md` › "Dialog state and order" (rows D1–D10);
 * each has a test in `sharing.test.ts`.
 *
 * In short: one dialog at a time, named by the opening that made it, so a
 * read for an earlier opening is let go (D3, D8). One change at a time, for
 * the window: while a share or revoke is on its way, none other begins, from
 * this dialog or one opened since (D6). A change settles whatever dialog is
 * open; if one shows its session it says the outcome and reads again (D5,
 * D7), and is held until that read answers (D10). Only the newest read is
 * applied (D2): the gateway's list says where things stand, never a guess
 * from the outcome. The draft lives here too, so moving it onto what the
 * person's own change left is decided by the read that follows, not by what
 * a render happened to see (D10).
 */
import {
  draftRebased,
  draftRestarted,
  emptyDraft,
  type ShareDraft,
  type SharingFailure,
  type SharingRead,
} from "../../model/sharing"
import type {
  ShareChange,
  ShareDialog,
  ShareSettled,
  WorkspaceState,
} from "../workspace-state"

/** D1: the dialog opens for one session, reading its grants, with an empty draft. */
export function shareOpened(
  state: WorkspaceState,
  { sessionId, opening, read }: { sessionId: string; opening: string; read: string },
): WorkspaceState {
  return {
    ...state,
    share: {
      sessionId,
      opening,
      value: null,
      reading: read,
      readFailure: null,
      notice: null,
      draft: emptyDraft(null),
      stale: null,
    },
  }
}

/** D7: closed. A read still on its way is let go when it comes; a change still settles. */
export function shareClosed(state: WorkspaceState): WorkspaceState {
  return state.share === null ? state : { ...state, share: null }
}

/** The open dialog, when `opening` is still the one that opened it. */
function current(state: WorkspaceState, opening: string): ShareDialog | null {
  return state.share?.opening === opening ? state.share : null
}

function withDialog(state: WorkspaceState, dialog: ShareDialog): WorkspaceState {
  return { ...state, share: dialog }
}

/** D2: a read is asked; a later one supersedes it. */
export function sharesReading(
  state: WorkspaceState,
  { opening, read }: { opening: string; read: string },
): WorkspaceState {
  const dialog = current(state, opening)
  return dialog ? withDialog(state, { ...dialog, reading: read }) : state
}

/**
 * D2, D3: the newest read of the open dialog is applied; any other is let
 * go. D10: after a change of its own, the draft moves onto what it left.
 */
export function sharesRead(
  state: WorkspaceState,
  { opening, read, value }: { opening: string; read: string; value: SharingRead },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog || dialog.reading !== read) return state
  return withDialog(state, {
    ...dialog,
    value,
    reading: null,
    readFailure: null,
    draft:
      dialog.stale && mayHaveMade(dialog.stale)
        ? draftRebased(dialog.draft, deviceOf(dialog.stale.change), value.grants)
        : dialog.draft,
    stale: null,
  })
}

/**
 * Whether a settled change may have been made: done, or with no answer to
 * say. A refusal made nothing, so a draft built on what was read before
 * still is.
 */
function mayHaveMade(settled: ShareSettled): boolean {
  return settled.outcome === "done" || settled.outcome === "unanswered"
}

/**
 * D4: the newest read failed. What was read before stays on screen beside
 * why; if a change settled since it was read, the form stays held (D10).
 */
export function sharesReadFailed(
  state: WorkspaceState,
  { opening, read, reason }: { opening: string; read: string; reason: SharingFailure },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog || dialog.reading !== read) return state
  return withDialog(state, { ...dialog, reading: null, readFailure: reason })
}

/**
 * Whether the form is held: a change is on its way, or one settled since
 * the dialog's grants were read. A change asked then might be built on what
 * is no longer so, and an edit would be moved by the read that follows, so
 * neither is taken.
 */
export function formHeld(state: WorkspaceState): boolean {
  return state.shareChange !== null || (state.share?.stale ?? null) !== null
}

/**
 * Whether the open dialog is held by a change to another session's sharing,
 * which it is not told the outcome of: so it can say why it waits.
 */
export function heldElsewhere(state: WorkspaceState): boolean {
  const change = state.shareChange
  return (
    change !== null && state.share !== null && change.sessionId !== state.share.sessionId
  )
}

/** The person set the draft, unless the form is held. */
export function shareDrafted(
  state: WorkspaceState,
  { opening, draft }: { opening: string; draft: ShareDraft },
): WorkspaceState {
  const dialog = current(state, opening)
  return dialog && !formHeld(state) ? withDialog(state, { ...dialog, draft }) : state
}

/**
 * D10: the person asked to start the draft again from what its device holds
 * now, after it changed under the draft.
 */
export function shareRestarted(
  state: WorkspaceState,
  { opening }: { opening: string },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog?.value) return state
  return shareDrafted(state, {
    opening,
    draft: draftRestarted(dialog.draft, dialog.value.grants),
  })
}

/** Whether the open dialog `opening` could begin a change now (D6). */
export function canChange(state: WorkspaceState, opening: string): boolean {
  const dialog = current(state, opening)
  return dialog !== null && !formHeld(state) && dialog.value !== null
}

/** D5: a change begins; the last one's notice goes. */
export function shareChanging(
  state: WorkspaceState,
  { opening, change }: { opening: string; change: ShareChange },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog || !canChange(state, opening)) return state
  return {
    ...state,
    shareChange: { sessionId: dialog.sessionId, change },
    share: { ...dialog, notice: null },
  }
}

/** The device a change names. */
function deviceOf(change: ShareChange): string {
  return change.kind === "share" ? change.request.credentialId : change.credentialId
}

/**
 * D5, D7: the change on its way settled, done or refused. It is no longer on
 * its way, whichever dialog began it. A dialog open on its session — the one
 * that began it, or one opened since — says its outcome, and is held until
 * the read that follows answers (D10). The command reads again for any such
 * dialog (`sharing-commands.ts`).
 */
export function shareChanged(
  state: WorkspaceState,
  { outcome }: { outcome: "done" | SharingFailure },
): WorkspaceState {
  const settled = state.shareChange
  if (settled === null) return state
  const next = { ...state, shareChange: null }
  const dialog = state.share
  if (dialog?.sessionId !== settled.sessionId) return next
  const said: ShareSettled = { change: settled.change, outcome }
  return withDialog(next, { ...dialog, notice: said, stale: said })
}
