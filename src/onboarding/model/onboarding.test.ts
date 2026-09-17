import { describe, expect, it } from "vitest"
import {
  AGENT_CHOICES,
  agentChoice,
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  isOnboarding,
  startAgentChoice,
} from "./onboarding"

describe("first-run setup", () => {
  it("starts on welcome and shows setup until it is finished", () => {
    const start = beginOnboarding()
    expect(start).toEqual({ step: "welcome" })
    expect(isOnboarding(start)).toBe(true)
    expect(isOnboarding({ step: "done", agent: "claude" })).toBe(false)
  })

  it("walks welcome to a chosen agent, then to summon, then finishes", () => {
    const picking = startAgentChoice(beginOnboarding())
    expect(picking.step).toBe("agent")
    const chosen = chooseAgent(picking, "claude")
    expect(chosen.agent).toBe("claude")
    const summon = confirmAgent(chosen)
    expect(summon).toEqual({ step: "summon", agent: "claude" })
    expect(isOnboarding(summon)).toBe(true)
    expect(completeOnboarding(summon)).toEqual({
      step: "done",
      agent: "claude",
    })
  })

  it("does not record an agent no provider can run", () => {
    const picking = startAgentChoice(beginOnboarding())
    expect(agentChoice("codex")?.available).toBe(false)
    expect(chooseAgent(picking, "codex")).toBe(picking)
  })

  it("does not leave the picker without a recorded agent", () => {
    const picking = startAgentChoice(beginOnboarding())
    expect(confirmAgent(picking)).toBe(picking)
    expect(isOnboarding(confirmAgent(picking))).toBe(true)
  })

  it("only finishes from the summon step", () => {
    const welcome = beginOnboarding()
    expect(completeOnboarding(welcome)).toBe(welcome)
    const chosen = chooseAgent(startAgentChoice(welcome), "claude")
    expect(completeOnboarding(chosen)).toBe(chosen)
  })

  it("ignores steps that do not apply to the current one", () => {
    const welcome = beginOnboarding()
    expect(chooseAgent(welcome, "claude")).toBe(welcome)
    const done = completeOnboarding(
      confirmAgent(chooseAgent(startAgentChoice(welcome), "claude")),
    )
    expect(startAgentChoice(done)).toBe(done)
    expect(chooseAgent(done, "claude")).toBe(done)
  })

  it("offers Claude first and marks every listed agent honestly", () => {
    expect(AGENT_CHOICES.map((choice) => choice.id)).toEqual(["claude", "codex"])
    expect(AGENT_CHOICES.filter((choice) => choice.available)).toHaveLength(1)
  })
})

describe("leaving setup without finishing it", () => {
  it("dismisses from any step and records no agent", () => {
    expect(dismissOnboarding(beginOnboarding())).toEqual({ step: "done" })
    const chosen = chooseAgent(startAgentChoice(beginOnboarding()), "claude")
    expect(dismissOnboarding(chosen)).toEqual({ step: "done" })
    expect(isOnboarding(dismissOnboarding(chosen))).toBe(false)
  })

  it("leaves a finished setup alone", () => {
    const done = completeOnboarding(
      confirmAgent(chooseAgent(startAgentChoice(beginOnboarding()), "claude")),
    )
    expect(dismissOnboarding(done)).toBe(done)
  })
})
