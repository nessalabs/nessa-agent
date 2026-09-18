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
    // The surface goes away and comes back — a remount, or the source being
    // swapped — which is the one way two asks are in flight at once.
    check.abandon()
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

  it("asks once while an ask is in flight, however many times it is asked", async () => {
    const source = heldSource()
    const received: AgentReadinessAnswer[] = []
    const check = createReadinessCheck(source, (answer) => received.push(answer))

    // Someone leaning on "Check again" while the probe is still running: each
    // press would be a subprocess per agent on macOS.
    const asks = [check.check(), check.check(), check.check()]
    expect(source.asked).toBe(1)

    source.answer(0, { ok: true, agents: { claude: "ready" } })
    await Promise.all(asks)

    // The presses that coalesced waited on the ask in flight rather than
    // resolving as if nothing had been asked.
    expect(received).toEqual([{ ok: true, agents: { claude: "ready" } }])
  })
})

describe("an ask and the ask before it", () => {
  it("asks again after an abandoned ask, while that one is still pending", async () => {
    const source = heldSource()
    const received: AgentReadinessAnswer[] = []
    const busy: boolean[] = []
    const check = createReadinessCheck(
      source,
      (answer) => received.push(answer),
      (value) => busy.push(value),
    )

    // Setup runs, is torn down, and runs again before the first probe answers —
    // what React's StrictMode does to every effect in development, and what an
    // `agents` identity change does at any time.
    const abandoned = check.check()
    check.abandon()
    const asked = check.check()

    // The second run really asked, rather than being refused as a duplicate of
    // the ask whose answer had already been thrown away.
    expect(source.asked).toBe(2)
    source.answer(1, { ok: true, agents: { claude: "ready" } })
    await asked
    expect(received).toEqual([{ ok: true, agents: { claude: "ready" } }])

    source.answer(0, { ok: false, reason: "unreachable" })
    await abandoned
    expect(received).toEqual([{ ok: true, agents: { claude: "ready" } }])
    expect(busy).toEqual([true, false, true, false])
  })

  it("does not let an older ask report that the current one has finished", async () => {
    const source = heldSource()
    const busy: boolean[] = []
    const check = createReadinessCheck(
      source,
      () => undefined,
      (value) => busy.push(value),
    )

    const abandoned = check.check()
    check.abandon()
    const asked = check.check()
    expect(busy).toEqual([true, false, true])

    // The abandoned probe finally comes back. The control belongs to the ask
    // that is still running, so it must stay disabled.
    source.answer(0, { ok: false, reason: "unreachable" })
    await abandoned
    expect(busy).toEqual([true, false, true])
    // And the ask in flight is still the one in flight: nothing new is started.
    void check.check()
    expect(source.asked).toBe(2)

    source.answer(1, { ok: true, agents: { claude: "ready" } })
    await asked
    expect(busy).toEqual([true, false, true, false])
  })

  it("reports busy for as long as the ask it started runs", async () => {
    const source = heldSource()
    const busy: boolean[] = []
    const check = createReadinessCheck(
      source,
      () => undefined,
      (value) => busy.push(value),
    )

    const asked = check.check()
    expect(busy).toEqual([true])
    void check.check()
    // A coalesced ask is not a second thing to wait for.
    expect(busy).toEqual([true])

    source.answer(0, { ok: true, agents: { claude: "ready" } })
    await asked
    expect(busy).toEqual([true, false])

    // Abandoning when nothing is outstanding has nothing to release.
    check.abandon()
    expect(busy).toEqual([true, false])
  })
})
