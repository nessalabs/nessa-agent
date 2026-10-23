import { strict as assert } from "node:assert"
import { test } from "node:test"

import { KNOBS, describe, knobsFrom, plan, random } from "./stress-plan.mjs"

test("the same numbers and seed give the same plan, and another seed another", () => {
  const knobs = knobsFrom({}, {})
  assert.deepEqual(plan(knobs), plan(knobs))
  assert.notDeepEqual(plan(knobs), plan({ ...knobs, seed: 2 }))
  assert.equal(random(7)(), random(7)())
})

test("every required step appears as often as its number says", () => {
  const knobs = knobsFrom({ rounds: "40", seed: "9" }, {})
  const steps = plan(knobs)
  const count = (kind) => steps.filter((step) => step.kind === kind).length
  assert.equal(count("model"), knobs.modelSwitches)
  assert.equal(count("stop"), knobs.stops)
  assert.equal(count("fail"), knobs.fails)
  assert.equal(count("usageLimit"), knobs.usageLimits)
  assert.equal(count("restart"), knobs.restarts)
  assert.equal(count("delegate"), knobs.threads)
  assert.equal(count("await"), knobs.awaits)
  assert.equal(count("aside"), knobs.asides)
  assert.equal(count("large"), knobs.large)
  assert.equal(count("park"), knobs.parks)
  assert.equal(count("rename"), knobs.renames)
  assert.equal(count("handoff"), knobs.handoffs)
  assert.equal(count("delete"), knobs.deletes)
  assert.equal(count("fresh"), knobs.fresh)
  assert.equal(count("window"), knobs.windows)
  assert.equal(count("linking"), knobs.linkings)
  assert.equal(count("rail"), knobs.rails)
  assert.equal(count("onboarding"), knobs.onboardings)
  assert.equal(count("table"), knobs.tables)
  assert.equal(count("trace"), knobs.traces)
  assert.equal(count("notice"), knobs.notices)
  assert.equal(count("pool"), knobs.pools)
  assert.equal(count("drift"), knobs.drifts)
  assert.equal(count("upgrade"), knobs.upgrades)
  assert.equal(count("launch"), knobs.launches)
  assert.equal(count("reviews"), 1)
  assert.equal(count("long"), 1)
  assert.equal(steps.find((step) => step.kind === "long").turns, knobs.long)
  assert.ok(count("switch") >= knobs.switches)
  assert.ok(steps.length >= 40)
  assert.deepEqual(
    steps.map((step) => step.index),
    steps.map((_, index) => index + 1),
  )
})

test("a failure is always followed by a reply, and a switch never picks the mode in force", () => {
  for (let seed = 0; seed < 50; seed += 1) {
    const steps = plan(knobsFrom({ seed: String(seed), fails: "3", switches: "6" }, {}))
    steps.forEach((step, index) => {
      if (step.kind === "fail" || step.kind === "usageLimit")
        assert.equal(steps[index + 1]?.kind, "reply")
      if (step.kind === "switch") {
        assert.notEqual(step.from, step.to)
        assert.ok(
          ["permission", "reviews", "large", "model"].includes(steps[index + 1]?.kind),
        )
        assert.equal(steps[index + 1]?.mode, step.to)
      }
      if (step.kind === "reviews" || step.kind === "large" || step.kind === "model")
        assert.notEqual(step.mode, "full")
    })
  }
})

test("no long conversation is planned when its number is 0", () => {
  const steps = plan(knobsFrom({ long: "0" }, {}))
  assert.ok(!steps.some((step) => step.kind === "long"))
})

test("numbers come from flags, then NESSA_STRESS_*, then defaults, and a bad one throws", () => {
  assert.equal(knobsFrom({ rounds: "30" }, { NESSA_STRESS_ROUNDS: "5" }).rounds, 30)
  assert.equal(knobsFrom({}, { NESSA_STRESS_ROUNDS: "5" }).rounds, 5)
  assert.equal(knobsFrom({}, {}).rounds, KNOBS.rounds.default)
  assert.throws(() => knobsFrom({ rounds: "-1" }, {}), /whole number/)
  assert.throws(() => knobsFrom({ seed: "x" }, {}), /whole number/)
  assert.match(describe(plan(knobsFrom({}, {}))), /^1\. /)
})

test("handoffs run last of all, after every upgrade, whatever the seed", () => {
  for (let seed = 0; seed < 50; seed += 1) {
    const steps = plan(knobsFrom({ seed: String(seed), handoffs: "2" }, {}))
    const first = steps.findIndex((step) => step.kind === "handoff")
    assert.equal(steps.filter((step) => step.kind === "handoff").length, 2)
    assert.ok(steps.slice(first).every((step) => step.kind === "handoff"))
    assert.ok(first > steps.findLastIndex((step) => step.kind === "upgrade"))
  }
})

test("positive model steps retain default and raised counts, run last but for handoffs, and enter Ask", () => {
  for (let seed = 0; seed < 50; seed += 1) {
    for (const count of [1, 6]) {
      const steps = plan(
        knobsFrom({ seed: String(seed), modelSwitches: String(count) }, {}),
      )
      const first = steps.findIndex((step) => step.kind === "model")
      assert.equal(steps.filter((step) => step.kind === "model").length, count)
      assert.ok(
        steps.slice(first).every((step) => ["model", "handoff"].includes(step.kind)),
      )
      let mode = "ask"
      for (const step of steps) {
        if (step.kind === "switch") {
          assert.equal(step.from, mode)
          mode = step.to
        }
        if (step.kind === "model") {
          assert.equal(step.mode, "ask")
          assert.equal(mode, "ask")
        }
      }
    }
  }
})

test("the legacy seed preserves the existing non-model work's relative order", () => {
  const steps = plan(
    knobsFrom(
      {
        windows: "0",
        linkings: "0",
        rails: "0",
        onboardings: "0",
        tables: "0",
        traces: "0",
        notices: "0",
        pools: "0",
        drifts: "0",
        upgrades: "0",
        launches: "0",
        awaits: "0",
        fresh: "0",
        renames: "0",
        deletes: "0",
        usageLimits: "0",
        handoffs: "0",
      },
      {},
    ),
  )
  // A forced Ask before a card has no following switch-witness permission.
  // Keep the original seed's required work and its existing witness turns.
  const original = steps.filter(
    (step, index) =>
      step.kind !== "model" &&
      !(step.kind === "switch" && steps[index + 1]?.after !== "switch"),
  )
  assert.deepEqual(
    original.map((step) => step.kind),
    [
      "delegate",
      "park",
      "stop",
      "long",
      "watch",
      "fail",
      "reply",
      "switch",
      "permission",
      "reviews",
      "switch",
      "permission",
      "restart",
      "aside",
      "large",
      "switch",
      "permission",
      "stop",
      "aside",
    ],
  )
})
