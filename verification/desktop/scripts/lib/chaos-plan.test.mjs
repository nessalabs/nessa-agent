import { strict as assert } from "node:assert"
import { test } from "node:test"

import { actions, describe, knobsFrom, shrink } from "./chaos-plan.mjs"

test("the same numbers and seed give the same actions, and another seed others", () => {
  const knobs = knobsFrom({}, {})
  assert.deepEqual(actions(knobs), actions(knobs))
  assert.notDeepEqual(actions(knobs), actions({ ...knobs, seed: 2 }))
})

test("every wave has its width, restarts are placed by their number, and every action names its place", () => {
  const knobs = knobsFrom({ waves: "20", width: "5", restarts: "3", seed: "4" }, {})
  const list = actions(knobs)
  const restarts = list.filter((action) => action.kind === "restart")
  assert.equal(restarts.length, 3)
  for (const action of restarts) assert.ok(action.wave > 0 && action.wave < 19)
  for (let wave = 0; wave < 20; wave += 1)
    assert.equal(
      list.filter((action) => action.wave === wave && action.kind !== "restart").length,
      5,
    )
  for (const action of list.filter((each) => each.kind !== "restart")) {
    assert.ok(action.slot >= 0 && action.slot < knobs.conversations)
    assert.ok(action.client >= 0 && action.client < knobs.clients)
  }
  assert.deepEqual(
    list.map((action) => action.index),
    list.map((_, index) => index + 1),
  )
  assert.equal(describe(list).split("\n").length, list.length)
})

test("a bad number is refused, from a flag or the environment", () => {
  assert.throws(() => knobsFrom({ waves: "-1" }, {}))
  assert.throws(() => knobsFrom({}, { NESSA_CHAOS_WIDTH: "x" }))
  assert.throws(() => knobsFrom({ conversations: "0" }, {}))
  assert.equal(knobsFrom({}, { NESSA_CHAOS_WAVES: "7" }).waves, 7)
})

test("shrinking keeps the fewest actions that still fail, in their order", async () => {
  const list = actions(knobsFrom({ waves: "10", width: "4" }, {}))
  const needed = [list[3].index, list[17].index]
  const fails = async (each) =>
    needed.every((index) => each.some((a) => a.index === index))
  const { list: small, tried } = await shrink(list, fails, 200)
  assert.deepEqual(
    small.map((action) => action.index),
    needed,
  )
  assert.ok(tried <= 200)
})

test("shrinking stops at its budget and keeps the input when no cut fails", async () => {
  const list = actions(knobsFrom({ waves: "4", width: "4" }, {}))
  let calls = 0
  const { list: kept, tried } = await shrink(
    list,
    async () => {
      calls += 1
      return false
    },
    5,
  )
  assert.equal(kept, list)
  assert.equal(tried, 5)
  assert.equal(calls, 5)
})

test("the default seeded actions include same-provider model controls", () => {
  assert.ok(actions(knobsFrom({}, {})).some((action) => action.kind === "model"))
})

test("a rename says what it types", () => {
  const renames = actions(knobsFrom({ waves: "60", width: "6" }, {})).filter(
    (action) => action.kind === "rename",
  )
  assert.ok(renames.length > 0)
  for (const action of renames)
    assert.ok(["plain", "folded", "long"].includes(action.title))
  assert.match(describe(renames), /rename (plain|folded|long) c\d/)
})
