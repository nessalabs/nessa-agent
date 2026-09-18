/**
 * What the setup window has to put on screen when the handoff did not end with
 * this window quietly gone.
 *
 * Two different failures reach this point and they are not the same news. A
 * panel that never came up leaves somebody with no Nessa at all, and the thing
 * to offer is the handoff again. A panel that came up over a window that would
 * not close leaves them with Nessa running behind a window in the way, and the
 * thing to offer is that window's close — retrying the handoff would summon a
 * panel that is already up and re-run a write already made.
 *
 * Reporting the second as the first is what this module exists to stop: the
 * screen said "Nessa could not open the panel" while the panel stood open
 * behind it.
 *
 * It is a function rather than branches inside the surface for the reason
 * `readiness-check.ts` and `dismiss-shortcut.ts` are: the decision is the part
 * worth testing, and a React effect that ends in a destroyed window is not
 * something a test can drive.
 */

import type { SetupHandoff } from "../../host"

/** What the recovery screen says, and which way out it offers. */
export interface SetupRecovery {
  /** The heading, which is the whole account of what happened. */
  heading: string
  /** The sentence under it: what is true now, and what to do about it. */
  detail: string
  /**
   * True when the panel is on screen and this window is the only thing left to
   * deal with. The screen then offers a close alone; there is nothing to hand
   * over again.
   */
  panelShown: boolean
}

/**
 * The screen a handoff leaves behind, or `null` when it leaves none.
 *
 * `undefined` is the handoff not having answered yet, which is not an ending.
 * An outcome this build does not know is treated as a panel that did not come
 * up: that screen offers both ways out, so it is the safe thing to be wrong
 * with.
 */
export function setupRecovery(handoff: SetupHandoff | undefined): SetupRecovery | null {
  if (handoff === undefined) return null
  switch (handoff.outcome) {
    // The window is closing, or there was never a window to close. Nothing to
    // recover from and nothing left to show.
    case "handed-over":
    case "no-native-host":
      return null
    case "setup-close-failed":
      return {
        heading: "Nessa is open behind this window",
        detail:
          "Setup finished and the panel is ready. This window would not close — try closing it again.",
        panelShown: true,
      }
    case "panel-unavailable":
    default:
      return {
        heading: "Nessa could not open the panel",
        detail:
          "Setup finished. Try again, or summon Nessa from the menu bar icon or with your summon shortcut.",
        panelShown: false,
      }
  }
}
