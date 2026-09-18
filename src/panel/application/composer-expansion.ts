import type { PillComposerExpansionReason } from "@nessa-ui/react/pill-composer"

/**
 * Whether the panel takes an expansion change the composer proposes.
 *
 * The composer offers a full-pane editor and asks to close it on every submit,
 * because sending is normally the end of writing. The panel does not always
 * send: an attachment still being read, a draft that is only files, an empty
 * editor — each turns a submit away, and the message the person wrote is still
 * sitting there. Collapsing then would take the pane out from under a draft
 * that never left, which is the one case where the pane is still earning its
 * place.
 *
 * Only a close asked for by a submit is weighed against whether that submit
 * sent anything. Minimize, Escape, and the composer withdrawing expansion
 * altogether say so in the reason and are always taken, so the ways out of the
 * pane keep working.
 */

/** True when the change should be applied. */
export function takesExpansion(
  next: boolean,
  reason: PillComposerExpansionReason,
  sent: boolean,
): boolean {
  return next || reason !== "submit" || sent
}
