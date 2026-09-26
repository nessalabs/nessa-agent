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
      "Couldn’t check your agents.",
    )
    expect(picker(recordReadiness(asking, { claude: "not-installed" }))).toContain(
      "Install or sign in to an agent.",
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
      step: "preparing",
    })

    expect(markup).toContain("starting nessa…")
    expect(markup).not.toContain("Can’t reach Nessa")
    expect(markup).not.toContain("Check again")
  })

  it.each([
    [
      "failed",
      {
        revision: 5,
        state: "failed",
        message: "Nessa’s background service could not be registered.",
      },
    ],
    [
      "could not be read",
      {
        state: "unavailable",
        message: "Nessa could not read its background service startup state.",
      },
    ],
  ] satisfies [string, GatewayStartupStatus][])(
    "shows only a retry, in place of the list, when startup %s",
    (_reason, startup) => {
      const markup = picker(recordReadiness(asking, { claude: "ready" }), startup)

      expect(markup).toContain("Nessa couldn’t start")
      expect(markup).toContain("Try starting Nessa again")
      // The host's diagnostic is for its log; setup does not recite it.
      expect(markup).not.toContain(startup.message)
      expect(markup).not.toContain("Choose an agent")
      expect(markup).not.toContain("Continue")
    },
  )
})

describe("an agent that cannot be picked", () => {
  it("names its reason, and keeps the words ready for hover and focus", () => {
    const markup = picker(recordReadiness(asking, { claude: "not-installed" }))
    expect(markup).toContain('aria-label="Claude, not installed"')
    // On screen when the row is hovered or focused, not only through a
    // tooltip a keyboard cannot open.
    expect(markup).toMatch(/group-focus:inline"[^>]*>Not installed</)
    expect(markup).not.toContain("title=")
  })
})

describe("an agent the icon set ships no mark for", () => {
  it("stands in a monogram rather than a drawing of somebody else's logo", () => {
    // Every agent ready, so no readiness mark adds paths of its own.
    const markup = picker(
      recordReadiness(asking, { claude: "ready", codex: "ready", opencode: "ready" }),
    )
    // The name is there, and so is the entry's tile — with a letter in it. A
    // fourth path element would mean a logo had been invented for it.
    expect(markup).toContain("OpenCode")
    expect(markup).toContain(">O</span>")
    expect(markup.match(/<path /g)).toHaveLength(2)
  })
})
