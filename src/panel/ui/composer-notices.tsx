import * as React from "react"

/**
 * Everything the composer says above the pill, in one box with a ceiling.
 *
 * Five producers write to this strip and none of them excludes the others: a
 * link that went nowhere, an available update, the draft's files, a sign-in
 * that could not be restored, and the conversation's own notice. Each is one
 * card at most, and each was correct on its own, but together they were an
 * unbounded column above a composer that cannot shrink — so on a short panel
 * the pill was pushed off the bottom of the window and the person could not
 * reach the thing they had opened the panel to use.
 *
 * The ceiling belongs here rather than to any one card. No card knows what the
 * others are saying, and a cap written into one of them would be that card
 * quietly taking ownership of the whole pane's layout. What this box owns is
 * the room the notices may have — a third of the panel, in `styles.css` — and
 * what happens when they want more, which is that the box scrolls. Nothing is
 * dropped, nothing is collapsed, and no notice changes what it says or when it
 * appears; they queue in a smaller window instead of pushing the composer out
 * of it.
 *
 * Because they scroll, which one is read first is now a decision, and this
 * component is where it is made. The order below is the order they are said
 * down the screen and it is the order that was already on screen; what is new
 * is that it is in one place, named, and pinned by a test, rather than being
 * whichever way five JSX blocks happened to be stacked in the panel's chrome.
 * A caller hands each notice to the slot it belongs in and cannot reorder them.
 *
 * The box is a tab stop, the way the transcript's scroller is: a card with
 * nothing to press has nothing inside it to tab to, so without that stop a
 * keyboard alone could not scroll past it. Cards that do have something to
 * press are still reached by tabbing, and the browser scrolls each one into
 * view as it takes focus — a Retry below the fold is one Tab away, not gone.
 *
 * Empty, it disappears entirely (`:empty` in `styles.css`), so a silent
 * composer has no stop in it and nothing to announce.
 */
export function ComposerNotices({
  link,
  update,
  attachments,
  session,
  conversation,
}: {
  /** A link that could not be opened, which is otherwise indistinguishable from a dead panel. */
  link: React.ReactNode
  /** A release waiting to be installed. */
  update: React.ReactNode
  /** What the draft's files need said, and why the last thing offered was turned away. */
  attachments: React.ReactNode
  /** A sign-in the surface around the session could not restore or end. */
  session: React.ReactNode
  /** The connection's own notice, and this conversation's. */
  conversation: React.ReactNode
}) {
  return (
    <div
      className="nessa-composer-notices outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      // Grouped and named rather than left as a bare focusable box: the stop
      // exists to scroll this strip, and a stop that announces nothing is a
      // stop nobody can account for.
      role="group"
      aria-label="Notices"
      tabIndex={0}
    >
      {link}
      {update}
      {attachments}
      {session}
      {conversation}
    </div>
  )
}
