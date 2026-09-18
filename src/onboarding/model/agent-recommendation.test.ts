import { describe, expect, it } from "vitest"
import { offersInstall, recommendAgent } from "./agent-recommendation"
import { beginOnboarding, recordReadiness, recordReadinessFailure } from "./onboarding"
import type { AgentId, AgentReadinessReport, OnboardingState } from "./onboarding"

/**
 * The agent these tests treat as the one Nessa can fetch.
 *
 * Claude, as a stand-in. Nessa does not install Claude — Opencode is the agent
 * this is being built for — but Opencode is not in the panel's listing yet, and
 * the module is deliberately not written around a name. Using a listed,
 * supported agent is what lets the offer path be tested today; swapping this
 * constant is the whole change when Opencode joins the listing.
 */
const INSTALLABLE: readonly AgentId[] = ["claude"]

/** Setup, having asked the runtimes and been told `report`. */
function asked(report: AgentReadinessReport): OnboardingState {
  return recordReadiness(beginOnboarding(), report)
}

describe("recommendAgent", () => {
  it("suggests nothing before the runtimes have answered", () => {
    // The picker has not asked yet. Offering a download here would be acting on
    // a question nobody has answered.
    expect(recommendAgent(beginOnboarding(), INSTALLABLE)).toEqual({ kind: "nothing" })
  })

  it("suggests nothing when the ask itself failed", () => {
    // A gateway that could not be reached is not a machine with no agents on
    // it. Someone whose gateway blipped should not be offered a large download
    // on the strength of it.
    const state = recordReadinessFailure(beginOnboarding(), "unreachable")
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "nothing" })
  })

  it("lets the person choose when an agent can start", () => {
    const state = asked({ claude: "ready", codex: "not-installed" })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "choose" })
  })

  it("lets the person choose when the installable agent is the ready one", () => {
    // Already installed and working. Offering to install it again would be
    // Nessa not knowing what is on the machine.
    const state = asked({ claude: "ready" })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "choose" })
  })

  it("offers the installable agent when nothing can start", () => {
    // The case this exists for: somebody who has never installed a coding
    // agent, who would otherwise be told to go away and come back.
    const state = asked({ claude: "not-installed", codex: "not-installed" })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({
      kind: "install",
      agent: "claude",
    })
  })

  it("offers an install when the agents present only need a sign-in", () => {
    // Installed but signed out is still nothing this person can use right now.
    const state = asked({ claude: "needs-authentication" })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({
      kind: "install",
      agent: "claude",
    })
  })

  it("offers nothing when Nessa can install nothing", () => {
    const state = asked({ claude: "not-installed", codex: "not-installed" })
    expect(recommendAgent(state, [])).toEqual({ kind: "nothing" })
  })

  it("offers nothing for an agent this build has no adapter for", () => {
    // Codex is listed but unsupported. A pinned release for an agent Nessa
    // cannot drive must not produce an offer that ends in a download and a row
    // that still cannot be selected.
    const state = asked({ claude: "not-installed" })
    expect(recommendAgent(state, ["codex"])).toEqual({ kind: "nothing" })
  })

  it("offers nothing for an agent setup does not list at all", () => {
    // The server's pins and the panel's listing are two different files.
    const state = asked({ claude: "not-installed" })
    expect(recommendAgent(state, ["nonesuch" as AgentId])).toEqual({ kind: "nothing" })
  })

  it("does not treat an unrecognised readiness as ready", () => {
    // Readiness states get added — the server has gained them before. Anything
    // that is not plainly `ready` is not somebody's working agent, so a new one
    // must not silently suppress the offer.
    const state = asked({ claude: "not-configured" as never })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({
      kind: "install",
      agent: "claude",
    })
  })

  it("skips an unofferable agent to reach one it can offer", () => {
    const state = asked({ claude: "not-installed" })
    const several = ["codex", "claude"] as readonly AgentId[]
    expect(recommendAgent(state, several)).toEqual({ kind: "install", agent: "claude" })
  })
})

describe("offersInstall", () => {
  it("is true only for an install", () => {
    expect(offersInstall({ kind: "install", agent: "claude" })).toBe(true)
    expect(offersInstall({ kind: "choose" })).toBe(false)
    expect(offersInstall({ kind: "nothing" })).toBe(false)
  })
})
