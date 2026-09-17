import { describe, expect, it } from "vitest"

import { recordsSetupCompletion } from "./setup-completion"
import {
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  recordReadiness,
  startAgentChoice,
  type OnboardingState,
} from "../model/onboarding"

/** The last step, reached the way a person reaches it. */
function atSummon(): OnboardingState {
  const ready = recordReadiness(beginOnboarding(), { claude: "ready" })
  return confirmAgent(chooseAgent(startAgentChoice(ready), "claude"))
}

const finished = completeOnboarding(atSummon())
const left = dismissOnboarding(atSummon())

describe("writing first-run setup off for good", () => {
  it("records a finish that put the panel on screen", () => {
    expect(recordsSetupCompletion(finished, "handed-over")).toBe(true)
  })

  it("records a finish on a surface with no second window", () => {
    // The browser path has no window to close: the panel renders where setup
    // was, which is as complete as completion gets there.
    expect(recordsSetupCompletion(finished, "no-native-host")).toBe(true)
  })

  it("does not record a finish whose panel never came up", () => {
    // What is on screen is the recovery screen. Closing it must leave first run
    // still to do, on a machine where nobody has seen the panel work.
    expect(recordsSetupCompletion(finished, "panel-unavailable")).toBe(false)
  })

  it("does not record a dismissal, however cleanly it handed over", () => {
    // Escape, the corner mark and the dimmed screen are as easily a slip as a
    // decision. Leaving stays free to change its mind.
    expect(recordsSetupCompletion(left, "handed-over")).toBe(false)
    expect(recordsSetupCompletion(left, "no-native-host")).toBe(false)
    expect(recordsSetupCompletion(left, "panel-unavailable")).toBe(false)
  })

  it("waits for the handoff to answer at all", () => {
    expect(recordsSetupCompletion(finished, undefined)).toBe(false)
  })

  it("records nothing while setup is still on screen", () => {
    expect(recordsSetupCompletion(atSummon(), "handed-over")).toBe(false)
    expect(recordsSetupCompletion(beginOnboarding(), "handed-over")).toBe(false)
  })
})
