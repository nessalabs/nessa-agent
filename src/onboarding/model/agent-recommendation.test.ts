import { describe, expect, it } from "vitest"
import { recommendAgent } from "./agent-recommendation"
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

  it("suggests nothing when the report says nothing about any agent", () => {
    // A report is an object, and an empty one is truthy. A gateway answering
    // `{"agents":[]}` — or one that has gained a readiness state this build
    // does not recognise, which the adapter drops rather than passes on — is
    // not a machine with no agents on it.
    expect(recommendAgent(asked({}), INSTALLABLE)).toEqual({ kind: "nothing" })
  })

  it("suggests nothing when the report names only agents setup does not list", () => {
    const state = asked({ nonesuch: "not-installed" } as AgentReadinessReport)
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "nothing" })
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

  it("treats every readiness that is not `ready` as nothing to start with", () => {
    // `not-supported` and `unknown` are part of the readiness union but never
    // come off the wire — the adapter refuses both, on the grounds that neither
    // is a fact a runtime gets to assert about itself. They reach this function
    // from Nessa's own side instead, so the offer has to be right for them
    // here.
    for (const readiness of ["not-supported", "unknown"] as const) {
      expect(
        recommendAgent(asked({ claude: readiness }), INSTALLABLE),
        `a ${readiness} agent is not one this person can start`,
      ).toEqual({ kind: "install", agent: "claude" })
    }
  })

  it("does not treat a readiness this build has not heard of as ready", () => {
    // The cast is the point: it stands in for a state added to the union after
    // this was written, which is the one case the compiler cannot flag here.
    // Not something the gateway can send today — the adapter drops a value it
    // does not know — so this is the second of the two places that hold the
    // rule, not the first.
    //
    // The rule being held is that only a plain `ready` suppresses the offer.
    // The opposite default would mean a state added on the server quietly
    // stopping setup from offering anything to someone with no agent at all.
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
