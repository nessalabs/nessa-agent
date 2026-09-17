import { describe, expect, it } from "vitest"
import {
  AGENT_CHOICES,
  agentChoice,
  agentReadiness,
  isChoosable,
  recordReadiness,
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  isOnboarding,
  pressSummon,
  startAgentChoice,
} from "./onboarding"

/** Setup with Claude reported ready, which is the only way it is choosable. */
function withClaudeReady(state = beginOnboarding()) {
  return recordReadiness(state, { claude: "ready", codex: "unavailable" })
}

function atSummon() {
  return confirmAgent(chooseAgent(startAgentChoice(withClaudeReady()), "claude"))
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
    const picking = startAgentChoice(withClaudeReady())
    expect(picking.step).toBe("agent")
    const chosen = chooseAgent(picking, "claude")
    expect(chosen.agent).toBe("claude")
    const summon = confirmAgent(chosen)
    // What the runtimes reported travels with the state: the picker is behind
    // us, but nothing has said it is no longer true.
    expect(summon).toEqual({
      step: "summon",
      agent: "claude",
      readiness: { claude: "ready", codex: "unavailable" },
    })
    expect(isOnboarding(summon)).toBe(true)
    // The shortcut lesson is the last step: finishing it finishes setup.
    expect(completeOnboarding(summonPressed())).toEqual({ step: "done", agent: "claude" })
  })

  it("does not record an agent no provider can run", () => {
    const picking = startAgentChoice(withClaudeReady())
    expect(agentChoice("codex")?.supported).toBe(false)
    expect(chooseAgent(picking, "codex")).toBe(picking)
  })

  it("offers nothing until the runtimes have been asked", () => {
    // Not asked yet is not the same as available, and only one of them is safe
    // to assume.
    const picking = startAgentChoice(beginOnboarding())
    expect(agentReadiness(picking, "claude")).toBe("unavailable")
    expect(isChoosable(picking, "claude")).toBe(false)
    expect(chooseAgent(picking, "claude")).toBe(picking)
  })

  it("does not offer an agent that is installed but not signed in", () => {
    const picking = recordReadiness(startAgentChoice(beginOnboarding()), {
      claude: "needs-authentication",
    })
    expect(agentReadiness(picking, "claude")).toBe("needs-authentication")
    expect(isChoosable(picking, "claude")).toBe(false)
    expect(chooseAgent(picking, "claude")).toBe(picking)
  })

  it("never offers an agent it has no adapter for, whatever is reported", () => {
    // A runtime cannot talk Nessa into running something it cannot drive.
    const picking = recordReadiness(startAgentChoice(beginOnboarding()), {
      codex: "ready",
    })
    expect(agentReadiness(picking, "codex")).toBe("unavailable")
    expect(chooseAgent(picking, "codex")).toBe(picking)
  })

  it("does not leave the picker without a recorded agent", () => {
    const picking = startAgentChoice(withClaudeReady())
    expect(confirmAgent(picking)).toBe(picking)
    expect(isOnboarding(confirmAgent(picking))).toBe(true)
  })

  it("only finishes from a completed shortcut lesson", () => {
    const welcome = beginOnboarding()
    expect(completeOnboarding(welcome)).toBe(welcome)
    const chosen = chooseAgent(startAgentChoice(withClaudeReady(welcome)), "claude")
    expect(completeOnboarding(chosen)).toBe(chosen)
    // Half a lesson is not a finished one.
    const summon = atSummon()
    expect(completeOnboarding(summon)).toBe(summon)
    const shown = pressSummon(summon)
    expect(completeOnboarding(shown)).toBe(shown)
  })

  it("ignores steps that do not apply to the current one", () => {
    const welcome = beginOnboarding()
    expect(chooseAgent(welcome, "claude")).toBe(welcome)
    const done = completeOnboarding(summonPressed())
    expect(startAgentChoice(done)).toBe(done)
    expect(chooseAgent(done, "claude")).toBe(done)
  })

  it("offers Claude first and marks every listed agent honestly", () => {
    expect(AGENT_CHOICES.map((choice) => choice.id)).toEqual(["claude", "codex"])
    expect(AGENT_CHOICES.map((choice) => choice.name)).toEqual(["Claude", "Codex"])
    expect(AGENT_CHOICES.filter((choice) => choice.supported)).toHaveLength(1)
  })
})

describe("learning the summon shortcut", () => {
  it("teaches both halves of the toggle before it offers a way out", () => {
    const summon = atSummon()
    expect(summon.summon).toBeUndefined()

    // Summoning is only half the lesson, so it is not yet a way out.
    const shown = pressSummon(summon)
    expect(shown.summon).toBe("shown")
    expect(completeOnboarding(shown)).toBe(shown)

    const hidden = pressSummon(shown)
    expect(hidden.summon).toBe("hidden")
    expect(completeOnboarding(hidden)).toEqual({ step: "done", agent: "claude" })
  })

  it("stops teaching once the lesson is over", () => {
    const hidden = summonPressed()
    expect(pressSummon(hidden)).toBe(hidden)
  })

  it("does not carry the lesson into the finished setup", () => {
    expect(completeOnboarding(summonPressed())).not.toHaveProperty("summon")
  })

  it("ignores presses outside the step that teaches it", () => {
    const welcome = beginOnboarding()
    expect(pressSummon(welcome)).toBe(welcome)
    const done = completeOnboarding(summonPressed())
    expect(pressSummon(done)).toBe(done)
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
    const done = completeOnboarding(summonPressed())
    expect(done.step).toBe("done")
    expect(dismissOnboarding(done)).toBe(done)
  })
})
