import type { PillComposerExpansionReason } from "@nessa-ui/react/pill-composer"

/**
 * Whether the panel takes an expansion change the composer proposes.
 *
 * The composer offers to close its full-pane editor on every submit, because
 * sending is normally the end of writing. Submitting is not sending: the
 * gateway may be away, the draft may be files only or larger than the gateway
 * accepts, and in each the message is still in the editor and still needs
 * somewhere to be read. The composer cannot know which happened — the call it
 * makes to send is the same call either way — so the offer is declined for
 * every submit and the panel closes the pane itself once the draft has
 * actually gone (see `useComposer`).
 *
 * Every other reason is the person leaving the pane, or the composer having to
 * take it away, and is always taken. Declining those would strand somebody
 * inside an editor whose exits had stopped working.
 */
export function takesExpansion(
  next: boolean,
  reason: PillComposerExpansionReason,
): boolean {
  return next || reason !== "submit"
}
