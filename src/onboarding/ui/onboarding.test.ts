import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { Onboarding } from "./onboarding"
import {
  beginOnboarding,
  chooseAgent,
  recordReadiness,
  recordReadinessFailure,
  startAgentChoice,
  type OnboardingState,
} from "../model/onboarding"

function picker(state: OnboardingState) {
  return renderToStaticMarkup(
    React.createElement(Onboarding, {
      state,
      platform: "apple" as const,
      onBegin: () => {},
      onChoose: () => {},
      onConfirm: () => {},
      onFinish: () => {},
      onRecheck: () => {},
    }),
  )
}

const asking = startAgentChoice(beginOnboarding())

describe("the agent picker with nothing to pick", () => {
  it.each([
    ["nobody answered", recordReadinessFailure(asking, "unreachable")],
    ["the answer was unreadable", recordReadinessFailure(asking, "unreadable")],
    [
      "Claude needs signing in",
      recordReadiness(asking, { claude: "needs-authentication" }),
    ],
    ["nothing is installed", recordReadiness(asking, { claude: "not-installed" })],
    ["no answer has arrived yet", asking],
  ])("offers another ask when %s", (_reason, state) => {
    const markup = picker(state)
    expect(markup).toContain("Check again")
    // A real button, so it is reachable by tab like every other control here.
    expect(markup).toContain("<button")
  })

  it("says what could not be asked apart from what answered", () => {
    expect(picker(recordReadinessFailure(asking, "unreachable"))).toContain(
      "Nessa could not ask what is installed here.",
    )
    expect(picker(recordReadiness(asking, { claude: "not-installed" }))).toContain(
      "No agent here can start yet.",
    )
  })

  it("does not ask again when an agent is ready to be picked", () => {
    const ready = recordReadiness(asking, { claude: "ready" })
    expect(picker(ready)).not.toContain("Check again")
    // And a choice made against that answer survives it being confirmed again.
    expect(chooseAgent(ready, "claude").agent).toBe("claude")
  })
})
