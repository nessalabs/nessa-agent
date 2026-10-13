/**
 * The share dialog's state, as pure transitions over the workspace
 * (nessa_ui#128, #129). The orderings are the table in
 * `docs/design/read-grants.md` › "Dialog state and order" (rows D1–D11);
 * each has a test in `sharing.test.ts`.
 *
 * In short: one dialog at a time, named by the opening that made it, so a
 * read for an earlier opening is let go (D3, D8). A read asks for the grants
 * and for what the form offers apart, so either can fail alone (D4). One
 * change at a time, for the window: while a share or revoke is on its way,
 * none other begins, from this dialog or one opened since (D6). A change
 * settles whatever dialog is open; if one shows its session it says the
 * outcome and reads again (D5, D7). Only the newest read is applied (D2).
 *
 * One rule says whether the list and the form may be acted on: `formHeld`
 * (D10). Every edit, Change, Revoke and its confirm, Start Again, Share and
 * the reason beside it derive from it, and nothing checks its parts apart.
 *
 * The draft's basis — the revision it builds on — is the gateway's answer to
 * this window's own change, never a later read (D10): a read can hold
 * another window's change, which the draft has not seen. A change built on
 * a revision the grant no longer holds is refused by the gateway (`changed`).
 */
import {
  draftRestarted,
  draftSettled,
  emptyDraft,
  type ShareDraft,
  type ShareGrant,
  type SharingChoices,
  type SharingFailure,
} from "../../model/sharing"
import type {
  ShareChange,
  ShareConfirming,
  ShareDialog,
  ShareReadFailure,
  WorkspaceState,
} from "../workspace-state"

/** One part of a read: what it answered, or why it did not. */
export type ReadAnswer<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly reason: SharingFailure }

/**
 * How a change settled. Made: the policy revision the device's grant holds
 * now, as the gateway answered it — `null` after a revoke, which leaves none.
 * Otherwise why not, which made nothing, or no answer, which may have.
 */
export type ChangeSettled =
  | { readonly outcome: "done"; readonly held: number | null }
  | { readonly outcome: SharingFailure }

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
      grants: null,
      choices: null,
      reading: read,
      readFailure: null,
      settled: null,
      confirming: null,
      draft: emptyDraft(null),
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
 * go. D4: each part that answered replaces what was read before; a part
 * that failed keeps it, and says why — the grants' failure first.
 */
export function sharesRead(
  state: WorkspaceState,
  {
    opening,
    read,
    grants,
    choices,
  }: {
    opening: string
    read: string
    grants: ReadAnswer<readonly ShareGrant[]>
    choices: ReadAnswer<SharingChoices>
  },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog || dialog.reading !== read) return state
  const readFailure: ShareReadFailure | null = !grants.ok
    ? { part: "grants", reason: grants.reason }
    : !choices.ok
      ? { part: "choices", reason: choices.reason }
      : null
  return withDialog(state, {
    ...dialog,
    grants: grants.ok ? grants.value : dialog.grants,
    choices: choices.ok ? choices.value : dialog.choices,
    reading: null,
    readFailure,
    settled:
      grants.ok && dialog.settled ? { ...dialog.settled, listed: true } : dialog.settled,
    confirming: grants.ok
      ? stillListed(dialog.confirming, grants.value)
      : dialog.confirming,
  })
}

/** A Revoke being confirmed, while the grants read still list it as it was asked. */
function stillListed(
  confirming: ShareConfirming | null,
  grants: readonly ShareGrant[],
): ShareConfirming | null {
  if (confirming === null) return null
  const listed = grants.find((grant) => grant.credentialId === confirming.credentialId)
  return listed?.policyRevision === confirming.policyRevision ? confirming : null
}

/**
 * D10: whether the list and the form are held — the one rule for it. They
 * are while a change is on its way, while a read is, and while the grants
 * listed predate this window's last settled change: until a read of the
 * grants answers after it, a share it made is not listed yet and a revoke's
 * device still is, so anything built on the list would be built on what the
 * change replaced. Only Try Again moves that on. While held no edit is
 * taken, no change or confirm begins, Start Again does nothing, and the
 * draft is not compared with the list, so no reason is said.
 */
export function formHeld(state: WorkspaceState): boolean {
  const dialog = state.share
  return (
    state.shareChange !== null ||
    (dialog !== null &&
      (dialog.reading !== null || (dialog.settled !== null && !dialog.settled.listed)))
  )
}

/**
 * D6: the change the open dialog waits on, so it can say why: one to this
 * session's sharing — the dialog's own, or one begun before it opened — or
 * to another session's, whose outcome it is not told.
 */
export function waitingOn(
  state: WorkspaceState,
): "this-session" | "another-session" | null {
  const change = state.shareChange
  if (change === null || state.share === null) return null
  return change.sessionId === state.share.sessionId ? "this-session" : "another-session"
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
 * as last read, after it changed under the draft.
 */
export function shareRestarted(
  state: WorkspaceState,
  { opening }: { opening: string },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog?.grants) return state
  return shareDrafted(state, {
    opening,
    draft: draftRestarted(dialog.draft, dialog.grants),
  })
}

/** Whether the open dialog `opening` could begin a change now (D6). */
export function canChange(state: WorkspaceState, opening: string): boolean {
  const dialog = current(state, opening)
  return dialog !== null && !formHeld(state) && dialog.grants !== null
}

/**
 * D11: Revoke asks first. What is asked about is the device's grant as
 * listed now; its revision is kept with the ask, and is what the revoke
 * names. Only when a change could begin, and for a device listed.
 */
export function shareConfirming(
  state: WorkspaceState,
  { opening, credentialId }: { opening: string; credentialId: string },
): WorkspaceState {
  const dialog = current(state, opening)
  const grant = dialog?.grants?.find((each) => each.credentialId === credentialId)
  if (!dialog || !grant || !canChange(state, opening)) return state
  return withDialog(state, {
    ...dialog,
    confirming: { credentialId, policyRevision: grant.policyRevision },
  })
}

/** The person said no to the Revoke they were asked about. */
export function shareConfirmCancelled(
  state: WorkspaceState,
  { opening }: { opening: string },
): WorkspaceState {
  const dialog = current(state, opening)
  return dialog?.confirming ? withDialog(state, { ...dialog, confirming: null }) : state
}

/** D5: a change begins; the last one's outcome goes, and so does any Revoke being asked (D11). */
export function shareChanging(
  state: WorkspaceState,
  { opening, change }: { opening: string; change: ShareChange },
): WorkspaceState {
  const dialog = current(state, opening)
  if (!dialog || !canChange(state, opening)) return state
  return {
    ...state,
    shareChange: { sessionId: dialog.sessionId, change },
    share: { ...dialog, settled: null, confirming: null },
  }
}

/** The device a change names. */
function deviceOf(change: ShareChange): string {
  return change.kind === "share" ? change.request.credentialId : change.credentialId
}

/**
 * D5, D7, D10: the change on its way settled. It is no longer on its way,
 * whichever dialog began it. A dialog open on its session — the one that
 * began it, or one opened since — says its outcome, starts the read `read`
 * that follows it (`sharing-commands.ts` asks it), and, when the change was
 * made, builds its draft on the revision the gateway answered.
 */
export function shareChanged(
  state: WorkspaceState,
  { settled, read }: { settled: ChangeSettled; read: string },
): WorkspaceState {
  const change = state.shareChange
  if (change === null) return state
  const next = { ...state, shareChange: null }
  const dialog = state.share
  if (dialog?.sessionId !== change.sessionId) return next
  return withDialog(next, {
    ...dialog,
    settled: { change: change.change, outcome: settled.outcome, listed: false },
    reading: read,
    draft:
      settled.outcome === "done"
        ? draftSettled(dialog.draft, deviceOf(change.change), settled.held)
        : dialog.draft,
  })
}
