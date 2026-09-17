import { describe, expect, it } from "vitest"
import {
  AGENT_CHOICES,
  agentChoice,
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  confirmSummon,
  dismissOnboarding,
  isOnboarding,
  pressSummon,
  startAgentChoice,
} from "./onboarding"

function atSummon() {
  return confirmAgent(chooseAgent(startAgentChoice(beginOnboarding()), "claude"))
}

/** The summon step with the shortcut pressed both times: shown, then hidden. */
function summonPressed() {
  return pressSummon(pressSummon(atSummon()))
}

describe("first-run setup", () => {
  it("starts on welcome and shows setup until it is finished", () => {
    const start = beginOnboarding()
    expect(start).toEqual({ step: "welcome" })
    expect(isOnboarding(start)).toBe(true)
    expect(isOnboarding({ step: "done", agent: "claude" })).toBe(false)
  })

  it("walks welcome to an agent, then the shortcut, then finishes", () => {
    const picking = startAgentChoice(beginOnboarding())
    expect(picking.step).toBe("agent")
    const chosen = chooseAgent(picking, "claude")
    expect(chosen.agent).toBe("claude")
    const summon = confirmAgent(chosen)
    expect(summon).toEqual({ step: "summon", agent: "claude" })
    const ready = confirmSummon(pressSummon(pressSummon(summon)))
    expect(ready).toEqual({ step: "ready", agent: "claude" })
    expect(isOnboarding(ready)).toBe(true)
    expect(completeOnboarding(ready)).toEqual({ step: "done", agent: "claude" })
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

  it("only finishes from the last step", () => {
    const welcome = beginOnboarding()
    expect(completeOnboarding(welcome)).toBe(welcome)
    const chosen = chooseAgent(startAgentChoice(welcome), "claude")
    expect(completeOnboarding(chosen)).toBe(chosen)
    expect(completeOnboarding(summonPressed())).toEqual(summonPressed())
  })

  it("ignores steps that do not apply to the current one", () => {
    const welcome = beginOnboarding()
    expect(chooseAgent(welcome, "claude")).toBe(welcome)
    const done = completeOnboarding(confirmSummon(summonPressed()))
    expect(startAgentChoice(done)).toBe(done)
    expect(chooseAgent(done, "claude")).toBe(done)
  })

  it("offers Claude first and marks every listed agent honestly", () => {
    expect(AGENT_CHOICES.map((choice) => choice.id)).toEqual(["claude", "codex"])
    expect(AGENT_CHOICES.map((choice) => choice.name)).toEqual(["Claude", "Codex"])
    expect(AGENT_CHOICES.filter((choice) => choice.available)).toHaveLength(1)
  })
})

describe("learning the summon shortcut", () => {
  it("teaches both halves of the toggle before it offers a way on", () => {
    const summon = atSummon()
    expect(summon.summon).toBeUndefined()
    expect(confirmSummon(summon)).toBe(summon)

    // Summoning is only half the lesson, so it is not yet a way on.
    const shown = pressSummon(summon)
    expect(shown.summon).toBe("shown")
    expect(confirmSummon(shown)).toBe(shown)

    const hidden = pressSummon(shown)
    expect(hidden.summon).toBe("hidden")
    expect(confirmSummon(hidden)).toEqual({ step: "ready", agent: "claude" })
  })

  it("stops teaching once the lesson is over", () => {
    const hidden = summonPressed()
    expect(pressSummon(hidden)).toBe(hidden)
  })

  it("does not carry the lesson into the step after it", () => {
    expect(confirmSummon(summonPressed())).not.toHaveProperty("summon")
  })

  it("ignores presses outside the step that teaches it", () => {
    const welcome = beginOnboarding()
    expect(pressSummon(welcome)).toBe(welcome)
    expect(confirmSummon(welcome)).toBe(welcome)
    const ready = confirmSummon(summonPressed())
    expect(pressSummon(ready)).toBe(ready)
    expect(confirmSummon(ready)).toBe(ready)
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
    const done = completeOnboarding(confirmSummon(summonPressed()))
    expect(done.step).toBe("done")
    expect(dismissOnboarding(done)).toBe(done)
  })
})
