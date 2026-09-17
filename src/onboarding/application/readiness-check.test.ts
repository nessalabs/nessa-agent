import { describe, expect, it } from "vitest"
import { createReadinessCheck } from "./readiness-check"
import type { AgentReadinessAnswer, AgentReadinessSource } from "./ports"

/** A source whose answers are released by hand, so two asks can be in flight. */
function heldSource(): AgentReadinessSource & {
  answer(index: number, value: AgentReadinessAnswer): void
  asked: number
} {
  const pending: ((value: AgentReadinessAnswer) => void)[] = []
  return {
    asked: 0,
    read() {
      this.asked += 1
      return new Promise<AgentReadinessAnswer>((resolve) => pending.push(resolve))
    },
    answer(index, value) {
      pending[index](value)
    },
  }
}

describe("asking the runtimes more than once", () => {
  it("keeps the newest answer when an older ask lands last", async () => {
    const source = heldSource()
    const received: AgentReadinessAnswer[] = []
    const check = createReadinessCheck(source, (answer) => received.push(answer))

    const first = check.check()
    const second = check.check()
    expect(source.asked).toBe(2)

    // The retry answers first: the gateway is up and Claude can start.
    source.answer(1, { ok: true, agents: { claude: "ready" } })
    await second
    // The original ask — from before anyone signed in — answers afterwards.
    source.answer(0, { ok: false, reason: "unreachable" })
    await first

    expect(received).toEqual([{ ok: true, agents: { claude: "ready" } }])
  })

  it("delivers each answer while the asks do not overlap", async () => {
    const source = heldSource()
    const received: AgentReadinessAnswer[] = []
    const check = createReadinessCheck(source, (answer) => received.push(answer))

    const first = check.check()
    source.answer(0, { ok: false, reason: "unreachable" })
    await first
    const second = check.check()
    source.answer(1, { ok: true, agents: { claude: "ready" } })
    await second

    expect(received).toEqual([
      { ok: false, reason: "unreachable" },
      { ok: true, agents: { claude: "ready" } },
    ])
  })

  it("drops an abandoned ask rather than writing it to a gone surface", async () => {
    const source = heldSource()
    const received: AgentReadinessAnswer[] = []
    const check = createReadinessCheck(source, (answer) => received.push(answer))

    const asked = check.check()
    check.abandon()
    source.answer(0, { ok: true, agents: { claude: "ready" } })
    await asked

    expect(received).toEqual([])
  })
})
