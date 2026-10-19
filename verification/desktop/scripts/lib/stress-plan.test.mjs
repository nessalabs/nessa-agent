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
  assert.equal(count("stop"), knobs.stops)
  assert.equal(count("fail"), knobs.fails)
  assert.equal(count("restart"), knobs.restarts)
  assert.equal(count("delegate"), knobs.threads)
  assert.equal(count("aside"), knobs.asides)
  assert.equal(count("large"), knobs.large)
  assert.equal(count("park"), knobs.parks)
  assert.equal(count("linking"), knobs.linkings)
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
      if (step.kind === "fail") assert.equal(steps[index + 1]?.kind, "reply")
      if (step.kind === "switch") {
        assert.notEqual(step.from, step.to)
        assert.ok(["permission", "reviews", "large"].includes(steps[index + 1]?.kind))
        assert.equal(steps[index + 1]?.mode, step.to)
      }
      if (step.kind === "reviews" || step.kind === "large")
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
