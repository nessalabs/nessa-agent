/**
 * What the setup window has to put on screen when the handoff did not end with
 * this window quietly gone.
 *
 * Three different failures reach this point and they are not the same news. A
 * panel that never came up leaves somebody with no Nessa at all, and the thing
 * to offer is the handoff again. A panel that came up over a window that would
 * not close leaves them with Nessa running behind a window in the way, and the
 * thing to offer is that window's close — retrying the handoff would summon a
 * panel that is already up and re-run a write already made. A panel that came
 * up over a write this machine refused leaves them with Nessa running and the
 * agent they just chose unrecorded, and the thing to offer is that write
 * again, alone.
 *
 * Reporting the second as the first is what this module exists to stop: the
 * screen said "Nessa could not open the panel" while the panel stood open
 * behind it. The third was reported as nothing at all — a console line, and a
 * window that closed itself before it could say anything.
 *
 * It is a function rather than branches inside the surface for the reason
 * `readiness-check.ts` and `dismiss-shortcut.ts` are: the decision is the part
 * worth testing, and a React effect that ends in a destroyed window is not
 * something a test can drive against the real host. The effects around this are
 * driven against a fake one in `ui/use-setup-handoff.test.ts`.
 */

import type { SetupHandoff } from "../../host"

/** What the recovery screen says, and which way out it offers. */
export interface SetupRecovery {
  /** The heading, which is the whole account of what happened. */
  heading: string
  /** The sentence under it: what is true now, and what to do about it. */
  detail: string
  /**
   * The one thing worth pressing besides closing the window, or `null` when
   * closing is all there is left to do.
   *
   * Named rather than a `panelShown` flag, which it used to be: that flag
   * answered "is the panel up" and was read as "is there anything to retry",
   * and the two stopped agreeing the moment a third ending existed where the
   * panel is up and there *is* something to ask for again.
   */
  offer: SetupRecoveryOffer | null
}

/** What the recovery screen can ask the host for. */
export type SetupRecoveryOffer =
  /** The whole handoff, from showing the panel. Only when it never came up. */
  | "hand-over-again"
  /** The write and the close alone, because the panel is already up. */
  | "save-again"

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
        offer: null,
      }
    // The panel is up and usable, so this is not an outage — but nothing was
    // written down, which costs the agent that was just chosen as well as the
    // straight start next time. Both facts belong on screen, because a person
    // who closes this window instead has lost the choice without being told.
    case "setup-not-recorded":
      return {
        heading: "Nessa is open, but setup was not saved",
        detail:
          "The panel is ready to use. This machine would not record that setup finished, so Nessa will ask again next time and this session will use its default agent. Try saving again.",
        offer: "save-again",
      }
    case "panel-unavailable":
    default:
      return {
        heading: "Nessa could not open the panel",
        detail:
          "Setup finished. Try again, or summon Nessa from the menu bar icon or with your summon shortcut.",
        offer: "hand-over-again",
      }
  }
}
