/**
 * When first-run setup is written off for good.
 *
 * Recording completion is the one decision here that a person cannot undo from
 * inside the product: the next launch opens straight into the panel and setup
 * never asks again. So it takes two facts, and neither of them is which
 * callback a surface happened to run.
 *
 * The first is the model's own account of how setup ended. Leaving setup —
 * Escape, the corner mark, a click on the dimmed screen behind a window that
 * covers the whole display — is a slip as easily as it is a decision, and it
 * stays free to change its mind.
 *
 * The second is what the handoff to the panel actually did. Setup that finished
 * into a panel that never came up is not a setup somebody got to the end of:
 * they are looking at a recovery screen. Writing it down there would bury first
 * run behind a failure the next launch cannot see.
 *
 * It lives here, out of the effect that performs the handoff, so the rule can be
 * read and tested as a rule — the same reason `readiness-check.ts` and
 * `dismiss-shortcut.ts` were lifted out of theirs.
 */

import type { SetupHandoff } from "../../host"
import { isOnboardingCompleted, type OnboardingState } from "../model/onboarding"

/** What the handoff to the panel reported, as the host names the outcomes. */
export type SetupHandoffOutcome = SetupHandoff["outcome"]

/**
 * Whether this ending should be persisted as "setup is done, never ask again".
 *
 * `undefined` is the handoff not having answered yet, which is not an ending.
 * Unknown outcomes are refused the same way as a failed one: the cost of a
 * mistaken yes is a first run nobody can get back to, and the cost of a
 * mistaken no is running setup once more.
 */
export function recordsSetupCompletion(
  state: OnboardingState,
  handoff: SetupHandoffOutcome | undefined,
): boolean {
  if (!isOnboardingCompleted(state)) return false
  switch (handoff) {
    // The panel is up and this window is closing: the thing setup was for.
    case "handed-over":
      return true
    // No second window to hand over to — the browser path renders the panel
    // where setup was. Nothing failed, and nothing is left to wait for.
    case "no-native-host":
      return true
    // The panel did not come up. What is on screen is the recovery screen, not
    // a finished setup, and closing it must leave first run still to do.
    case "panel-unavailable":
      return false
    default:
      return false
  }
}
