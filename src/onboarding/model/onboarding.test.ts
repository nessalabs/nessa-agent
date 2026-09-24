import { describe, expect, it } from "vitest"
import {
  AGENT_CHOICES,
  agentChoice,
  agentReadiness,
  isChoosable,
  recordReadiness,
  recordReadinessFailure,
  beginOnboarding,
  clearReadiness,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  isOnboarding,
  isOnboardingCompleted,
  pressSummon,
  startAgentChoice,
  type AgentId,
} from "./onboarding"

/** Setup with Claude reported ready, which is the only way it is choosable. */
function withClaudeReady(state = beginOnboarding()) {
  return recordReadiness(state, { claude: "ready", codex: "not-installed" })
}

function atSummon() {
  return confirmAgent(chooseAgent(startAgentChoice(withClaudeReady()), "claude"))
}

/** The summon step with the shortcut reported both ways: shown, then hidden. */
function summonPressed() {
  return pressSummon(pressSummon(atSummon(), true), false)
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
      readiness: { claude: "ready", codex: "not-installed" },
      readinessFailure: undefined,
    })
    expect(isOnboarding(summon)).toBe(true)
    // The shortcut lesson is the last step: finishing it finishes setup.
    expect(completeOnboarding(summonPressed())).toEqual({
      step: "done",
      outcome: "completed",
      agent: "claude",
    })
  })

  it("does not record an agent this machine has not said is ready", () => {
    // Codex has an adapter now, so what stands between it and being picked is
    // what the machine reported — here, that it is not on this one.
    const picking = startAgentChoice(withClaudeReady())
    expect(agentChoice("codex")?.supported).toBe(true)
    expect(agentReadiness(picking, "codex")).toBe("not-installed")
    expect(chooseAgent(picking, "codex")).toBe(picking)
  })

  it("drops a choice when the managed gateway starts again", () => {
    const chosen = chooseAgent(startAgentChoice(withClaudeReady()), "claude")

    expect(clearReadiness(chosen)).toEqual({
      step: "agent",
      agent: undefined,
      readiness: undefined,
      readinessFailure: undefined,
    })
  })

  it("offers nothing until the runtimes have been asked", () => {
    // Not asked yet is not the same as available, and only one of them is safe
    // to assume.
    const picking = startAgentChoice(beginOnboarding())
    expect(agentReadiness(picking, "claude")).toBe("unknown")
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

  it("offers the second agent on the same terms as the first", () => {
    const picking = recordReadiness(startAgentChoice(beginOnboarding()), {
      codex: "ready",
    })
    expect(agentReadiness(picking, "codex")).toBe("ready")
    expect(chooseAgent(picking, "codex").agent).toBe("codex")
  })

  it("never offers an agent it has no adapter for, whatever is reported", () => {
    // A gateway newer than this panel can report an agent the listing does not
    // have, which is why the cast is here: the type says what this build lists,
    // and the wire does not have to agree. A runtime cannot talk Nessa into
    // running something it cannot drive.
    const unlisted = "gemini" as AgentId
    const picking = recordReadiness(startAgentChoice(beginOnboarding()), {
      [unlisted]: "ready",
    })
    expect(agentReadiness(picking, unlisted)).toBe("not-supported")
    expect(chooseAgent(picking, unlisted)).toBe(picking)
  })

  it("does not leave the picker without a recorded agent", () => {
    const picking = startAgentChoice(withClaudeReady())
    expect(confirmAgent(picking)).toBe(picking)
    expect(isOnboarding(confirmAgent(picking))).toBe(true)
  })

  it("only finishes from the last step", () => {
    const welcome = beginOnboarding()
    expect(completeOnboarding(welcome)).toBe(welcome)
    const chosen = chooseAgent(startAgentChoice(withClaudeReady(welcome)), "claude")
    expect(completeOnboarding(chosen)).toBe(chosen)
  })

  it("finishes from the summon step with the lesson unlearned", () => {
    // Nothing else can: a configuration that registers no accelerator has
    // nothing to press, and refusing to finish left setup with no way out but
    // abandoning it, which records nothing and starts over next launch.
    expect(completeOnboarding(atSummon())).toEqual({
      step: "done",
      outcome: "completed",
      agent: "claude",
    })
  })

  it("ignores steps that do not apply to the current one", () => {
    const welcome = beginOnboarding()
    expect(chooseAgent(welcome, "claude")).toBe(welcome)
    const done = completeOnboarding(summonPressed())
    expect(startAgentChoice(done)).toBe(done)
    expect(chooseAgent(done, "claude")).toBe(done)
  })

  it("offers Claude first and marks every listed agent honestly", () => {
    expect(AGENT_CHOICES.map((choice) => choice.id)).toEqual([
      "claude",
      "codex",
      "opencode",
    ])
    expect(AGENT_CHOICES.map((choice) => choice.name)).toEqual([
      "Claude",
      "Codex",
      "OpenCode",
    ])
    // All three have an adapter now. What is left between a listed agent and
    // being picked is what this machine reports about it.
    expect(AGENT_CHOICES.every((choice) => choice.supported)).toBe(true)
  })

  it("treats readiness as authoritative for every listed agent", () => {
    // OpenCode is choosable because the current gateway observation said
    // `ready`, by the same rule as the other two. The picker does not infer a
    // credential or a provider entitlement of its own.
    const picking = recordReadiness(startAgentChoice(beginOnboarding()), {
      opencode: "ready",
    })
    expect(agentChoice("opencode")?.supported).toBe(true)
    expect(agentReadiness(picking, "opencode")).toBe("ready")
    expect(chooseAgent(picking, "opencode").agent).toBe("opencode")
    // A prior ready answer does not make an absent runtime pickable.
    const missing = recordReadiness(picking, { opencode: "not-installed" })
    expect(chooseAgent(missing, "opencode").agent).toBeUndefined()
  })
})

describe("learning the summon shortcut", () => {
  it("teaches both halves of the toggle before it offers a way out", () => {
    const summon = atSummon()
    expect(summon.summon).toBeUndefined()
    expect(summon.summonTaught).toBeUndefined()

    // Summoning is only half the lesson, so it is not yet a way out.
    const shown = pressSummon(summon, true)
    expect(shown.summon).toBe("shown")
    expect(shown.summonTaught).toBe(false)

    const hidden = pressSummon(shown, false)
    expect(hidden.summon).toBe("hidden")
    expect(hidden.summonTaught).toBe(true)
  })

  it("records what the host reported, not which press this was", () => {
    // The panel can already be on screen when the step is reached — toggled
    // from the tray, or summoned earlier while nothing was listening. The first
    // press then *hides* it. A press counter said "shown" and told the person
    // to press it again to hide a panel that was already gone.
    const first = pressSummon(atSummon(), false)
    expect(first.summon).toBe("hidden")

    const second = pressSummon(first, true)
    expect(second.summon).toBe("shown")

    const third = pressSummon(second, false)
    expect(third.summon).toBe("hidden")
  })

  it("does not un-teach the lesson when the panel comes back", () => {
    // A third press is somebody trying it out. Taking the way on back off the
    // screen for it would be a punishment for curiosity.
    const again = pressSummon(summonPressed(), true)
    expect(again.summon).toBe("shown")
    expect(again.summonTaught).toBe(true)
  })

  it("changes nothing when the host repeats what is already recorded", () => {
    const hidden = summonPressed()
    expect(pressSummon(hidden, false)).toBe(hidden)
  })

  it("does not carry the lesson into the finished setup", () => {
    const done = completeOnboarding(summonPressed())
    expect(done).not.toHaveProperty("summon")
    expect(done).not.toHaveProperty("summonTaught")
  })

  it("ignores presses outside the step that teaches it", () => {
    const welcome = beginOnboarding()
    expect(pressSummon(welcome, true)).toBe(welcome)
    const done = completeOnboarding(summonPressed())
    expect(pressSummon(done, true)).toBe(done)
  })
})

describe("what the runtimes answered", () => {
  it("tells a failed ask apart from an agent with no adapter", () => {
    const asked = recordReadinessFailure(
      startAgentChoice(beginOnboarding()),
      "unreachable",
    )
    // Not a fact about Claude: nobody answered for it.
    expect(agentReadiness(asked, "claude")).toBe("unknown")
    expect(asked.readinessFailure).toBe("unreachable")
    // Which is a different thing from an agent this build cannot drive, about
    // which there is an answer whatever any gateway says.
    expect(agentReadiness(asked, "gemini" as AgentId)).toBe("not-supported")
    expect(isChoosable(asked, "claude")).toBe(false)
  })

  it("keeps an unreadable answer distinct from an unreachable one", () => {
    const asked = recordReadinessFailure(beginOnboarding(), "unreadable")
    expect(asked.readinessFailure).toBe("unreadable")
  })

  it("drops a stale report when a later ask fails", () => {
    // A "ready" from before the gateway went away is not evidence it can start.
    const failed = recordReadinessFailure(withClaudeReady(), "unreachable")
    expect(failed.readiness).toBeUndefined()
    expect(agentReadiness(failed, "claude")).toBe("unknown")
  })

  it("unpicks an agent a later answer says cannot run", () => {
    // The picker can ask again, so an answer can contradict the one a choice
    // was made against. Leaving it ticked, with Continue still lit, offers an
    // agent the runtime has just said is not there.
    const chosen = chooseAgent(startAgentChoice(withClaudeReady()), "claude")
    expect(chosen.agent).toBe("claude")
    const signedOut = recordReadiness(chosen, { claude: "needs-authentication" })
    expect(signedOut.agent).toBeUndefined()
    expect(isChoosable(signedOut, "claude")).toBe(false)
    // A failed ask is no better a reason to keep it: nobody answered at all.
    expect(recordReadinessFailure(chosen, "unreachable").agent).toBeUndefined()
  })

  it("keeps a choice a later answer still supports", () => {
    const chosen = chooseAgent(startAgentChoice(withClaudeReady()), "claude")
    expect(recordReadiness(chosen, { claude: "ready" }).agent).toBe("claude")
  })

  it("does not empty a decision behind a step that has moved on", () => {
    // Past the picker there is nothing on screen about agents, so unmaking the
    // choice silently would be the worse of the two lies.
    const summon = confirmAgent(
      chooseAgent(startAgentChoice(withClaudeReady()), "claude"),
    )
    expect(summon.step).toBe("summon")
    expect(recordReadinessFailure(summon, "unreachable").agent).toBe("claude")
  })

  it("clears the failure once an answer arrives", () => {
    const answered = recordReadiness(
      recordReadinessFailure(beginOnboarding(), "unreachable"),
      { claude: "ready" },
    )
    expect(answered.readinessFailure).toBeUndefined()
    expect(agentReadiness(answered, "claude")).toBe("ready")
  })
})

describe("leaving setup without finishing it", () => {
  it("dismisses from any step and records no agent", () => {
    expect(dismissOnboarding(beginOnboarding())).toEqual({
      step: "done",
      outcome: "dismissed",
    })
    const chosen = chooseAgent(startAgentChoice(beginOnboarding()), "claude")
    expect(dismissOnboarding(chosen)).toEqual({ step: "done", outcome: "dismissed" })
    expect(isOnboarding(dismissOnboarding(chosen))).toBe(false)
  })

  it("leaves a finished setup alone", () => {
    const done = completeOnboarding(summonPressed())
    expect(done.step).toBe("done")
    expect(dismissOnboarding(done)).toBe(done)
    // Including the record of how it ended: a dismissal arriving after the
    // finish must not downgrade a completion to a walk-out.
    expect(isOnboardingCompleted(dismissOnboarding(done))).toBe(true)
  })

  it("says which of the two endings reached done", () => {
    // Both land on the same step, and only one of them is somebody having
    // chosen. Anything that writes setup off for good reads this rather than
    // guessing from which way the surface got here.
    const finished = completeOnboarding(summonPressed())
    const left = dismissOnboarding(atSummon())
    expect(finished.step).toBe(left.step)
    expect(finished.outcome).toBe("completed")
    expect(left.outcome).toBe("dismissed")
    expect(isOnboardingCompleted(finished)).toBe(true)
    expect(isOnboardingCompleted(left)).toBe(false)
  })

  it("does not read an unfinished step, or an outcome-less done, as finished", () => {
    expect(isOnboardingCompleted(beginOnboarding())).toBe(false)
    expect(isOnboardingCompleted(atSummon())).toBe(false)
    // A done with nothing recorded about how it got there is not evidence that
    // anybody chose anything.
    expect(isOnboardingCompleted({ step: "done", agent: "claude" })).toBe(false)
  })
})
