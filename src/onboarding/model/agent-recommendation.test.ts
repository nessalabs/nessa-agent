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

  it("does not offer to install an agent that is installed and signed out", () => {
    // The near miss. `needs-authentication` looks like a problem an offer could
    // solve and is not: the agent is already on the machine, so the install
    // would find the pinned version, fetch nothing, and leave the row as
    // unselectable as before — having told somebody their sign-in problem was a
    // missing install.
    const state = asked({ claude: "needs-authentication" })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "nothing" })
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

  it("offers nothing on a readiness that is the absence of an answer", () => {
    // `unknown` and `not-supported` are part of the union but never come off
    // the wire — the adapter refuses both, on the grounds that neither is a
    // fact a runtime gets to assert about itself. They reach this function from
    // Nessa's own side, and neither is a runtime saying it cannot start, so
    // neither is grounds for proposing a download.
    for (const readiness of ["unknown", "not-supported"] as const) {
      expect(
        recommendAgent(asked({ claude: readiness }), INSTALLABLE),
        `${readiness} is not an agent reporting that it cannot start`,
      ).toEqual({ kind: "nothing" })
    }
  })

  it("offers nothing on a readiness this build has not heard of", () => {
    // The cast is the point: it stands in for a state added to the union after
    // this was written, which is the one case the compiler cannot flag here.
    // Not something the gateway can send today — the adapter drops a value it
    // does not know — so this is the second of the two places holding the rule.
    //
    // A new state is not a runtime saying it cannot start. Reading it as one
    // would mean a readiness added on the server quietly starting to propose a
    // hundred-megabyte download to people who did not need it.
    const state = asked({ claude: "not-configured" as never })
    expect(recommendAgent(state, INSTALLABLE)).toEqual({ kind: "nothing" })
  })

  it("offers nothing for an agent nobody answered about", () => {
    // The defect this guards: a global "did anybody answer" check passes on
    // codex's answer, and the offer is then made about claude, which nothing
    // was said about. Somebody whose claude is installed and working — but
    // whose entry the adapter dropped, because its readiness spelling is one
    // this build does not recognise — would be offered a download for it.
    expect(recommendAgent(asked({ codex: "not-installed" }), INSTALLABLE)).toEqual({
      kind: "nothing",
    })
    expect(recommendAgent(asked({ codex: "ready" }), INSTALLABLE)).toEqual({
      kind: "nothing",
    })
  })

  it("skips an unofferable agent to reach one it can offer", () => {
    const state = asked({ claude: "not-installed" })
    const several = ["codex", "claude"] as readonly AgentId[]
    expect(recommendAgent(state, several)).toEqual({ kind: "install", agent: "claude" })
  })
})
