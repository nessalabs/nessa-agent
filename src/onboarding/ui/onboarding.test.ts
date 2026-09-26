import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { Onboarding } from "./onboarding"
import type { GatewayStartupStatus } from "../../startup/application/gateway-startup"
import {
  beginOnboarding,
  chooseAgent,
  recordReadiness,
  recordReadinessFailure,
  startAgentChoice,
  type OnboardingState,
} from "../model/onboarding"

function picker(
  state: OnboardingState,
  gatewayStartup: GatewayStartupStatus = { revision: 0, state: "unmanaged" },
) {
  return renderToStaticMarkup(
    React.createElement(Onboarding, {
      state,
      gatewayStartup,
      platform: "apple" as const,
      onBegin: () => {},
      onChoose: () => {},
      onConfirm: () => {},
      onFinish: () => {},
      onRecheck: () => {},
      onRetryGateway: () => {},
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

describe("the managed gateway while setup is open", () => {
  it("shows startup as progress rather than an agent failure", () => {
    // Even a retained agent answer cannot outrank the native owner's newer
    // statement that the gateway is starting again.
    const markup = picker(recordReadiness(asking, { claude: "ready" }), {
      revision: 4,
      state: "starting",
      step: "replacing",
    })

    expect(markup).toContain("Starting Nessa…")
    expect(markup).toContain("Finishing the last update…")
    expect(markup).not.toContain("Can’t reach Nessa")
    expect(markup).not.toContain("Check again")
  })

  it("shows the host failure and an explicit retry", () => {
    const markup = picker(asking, {
      revision: 5,
      state: "failed",
      message: "Nessa’s background service could not be registered.",
    })

    expect(markup).toContain("Nessa needs attention")
    expect(markup).toContain("Nessa couldn’t start.")
    // The host's own words are kept, folded behind Details, for whoever helps.
    expect(markup).toContain("<summary")
    expect(markup).toContain("Nessa’s background service could not be registered.")
    expect(markup).toContain("Try starting Nessa again")
    expect(markup).not.toContain("Check again")
  })

  it("keeps an unreadable startup state distinct from a confirmed failure", () => {
    const markup = picker(asking, {
      state: "unavailable",
      message: "Nessa could not read its background service startup state.",
    })

    expect(markup).toContain("Can’t check Nessa startup")
    expect(markup).toContain("Nessa couldn’t check whether it started.")
    expect(markup).toContain("Nessa could not read its background service startup state.")
    expect(markup).not.toContain("Nessa needs attention")
  })
})

describe("an agent the icon set ships no mark for", () => {
  it("stands in a monogram rather than a drawing of somebody else's logo", () => {
    const markup = picker(recordReadiness(asking, { opencode: "ready" }))
    // The name is there, and so is the entry's tile — with a letter in it. A
    // fourth path element would mean a logo had been invented for it.
    expect(markup).toContain("OpenCode")
    expect(markup).toContain(">O</span>")
    expect(markup.match(/<path /g)).toHaveLength(2)
  })
})
