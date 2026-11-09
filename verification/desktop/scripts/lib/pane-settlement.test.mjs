import { strict as assert } from "node:assert"
import { test } from "node:test"
import { JSDOM } from "jsdom"
import { errors } from "playwright"
import { CannotRun } from "./cli.mjs"
import {
  expectedPaneFocus,
  observePaneFocus,
  paneFocusSettled,
  requirePendingMovement,
} from "./pane-settlement.mjs"
import { css } from "./selectors.mjs"

function fixture(t, expected) {
  const dom = new JSDOM("<main></main>")
  const previous = globalThis.document
  globalThis.document = dom.window.document
  t.after(() => {
    globalThis.document = previous
    dom.window.close()
  })
  let animations = []
  document.getAnimations = () => animations
  const paint = ({ keys = ["old"], marked = ["old"], caret = "old", blocked } = {}) => {
    document.querySelector("main").innerHTML = keys
      .map(
        (key) =>
          `<section data-pane-key="${key}" ${marked.includes(key) ? "data-pane-focused" : ""}><textarea></textarea></section>`,
      )
      .join("")
    const field = document.querySelector(`[data-pane-key="${caret}"] textarea`)
    field?.focus()
    if (blocked) field?.parentElement.setAttribute(blocked, "")
  }
  paint()
  return {
    paint,
    motion(value) {
      animations = value
    },
    owner: observePaneFocus({ selectors: css, expected }),
  }
}

test("new pane waits through mounted mark and late caret, retaining the old pane", (t) => {
  const { owner, paint } = fixture(t, { keys: ["old"], added: true })
  assert.equal(owner.ready(), false, "old mark and old caret cannot finish a split")
  paint({ keys: ["old", "new"], marked: ["new"], caret: "old" })
  for (let frame = 3; frame < 6; frame++) assert.equal(owner.ready(), false)
  paint({ keys: ["old", "new"], marked: ["new"], caret: "new" })
  assert.equal(owner.ready(), true)
  assert.deepEqual(
    owner.report().trace.map((row) => row.activeInPane),
    ["old", "old", "old", "old", "new"],
  )
})

for (const [name, state] of [
  ["old pane remains focused", { keys: ["old", "new"], marked: ["old"], caret: "old" }],
  ["old pane disappears", { keys: ["new"], marked: ["new"], caret: "new" }],
  [
    "two panes are added",
    { keys: ["old", "new", "extra"], marked: ["new"], caret: "new" },
  ],
  [
    "duplicate focus marks",
    { keys: ["old", "new"], marked: ["old", "new"], caret: "new" },
  ],
  ["no composer caret", { keys: ["old", "new"], marked: ["new"], caret: "absent" }],
]) {
  test(`split refuses ${name}`, (t) => {
    const { owner, paint } = fixture(t, { keys: ["old"], added: true })
    paint(state)
    assert.equal(owner.ready(), false)
  })
}

for (const blocked of [
  "inert",
  "data-drag-reflow",
  "data-split-flying",
  "data-split-measuring",
  "data-message-arrival-waiting",
]) {
  test(`focus waits while its composer is ${blocked}`, (t) => {
    const { owner, paint } = fixture(t, { keys: ["old"], target: "old" })
    paint({ blocked })
    assert.equal(owner.ready(), false)
    paint()
    assert.equal(owner.ready(), true)
  })
}

test("finite movement must finish; infinite ambient animation does not block focus", (t) => {
  const { owner, motion } = fixture(t, { keys: ["old"], target: "old" })
  for (const playState of ["running", "pending"]) {
    motion([{ playState, effect: { getTiming: () => ({ iterations: 1 }) } }])
    assert.equal(owner.ready(), false)
  }
  motion([
    { playState: "running", effect: { getTiming: () => ({ iterations: Infinity }) } },
  ])
  assert.equal(owner.ready(), true)
})

test("pane navigation waits for the commanded destination", (t) => {
  const { owner, paint } = fixture(t, { keys: ["old", "second"], target: "second" })
  paint({ keys: ["old", "second"] })
  assert.equal(owner.ready(), false)
  paint({ keys: ["old", "second"], marked: ["second"], caret: "second" })
  assert.equal(owner.ready(), true)
})

test("a button in the right pane does not stand in for its composer", (t) => {
  const { owner } = fixture(t, { keys: ["old"], target: "old" })
  const button = document.createElement("button")
  document.querySelector("section").append(button)
  button.focus()
  assert.equal(owner.ready(), false)
})

test("close must remove exactly the formerly focused pane", (t) => {
  const { owner, paint } = fixture(t, { keys: ["old"] })
  paint({ keys: ["old", "closed"] })
  assert.equal(owner.ready(), false)
  paint()
  assert.equal(owner.ready(), true)
})

test("destinations follow pane reading order and retain boundary navigation", () => {
  const before = { keys: ["one", "two", "three"], focusedPane: "two" }
  for (const [move, target] of [
    ["same", "two"],
    [0, "one"],
    [2, "three"],
    ["next", "three"],
    ["previous", "one"],
  ])
    assert.deepEqual(expectedPaneFocus(before, move), { keys: before.keys, target })
  assert.deepEqual(expectedPaneFocus(before, "new"), { keys: before.keys, added: true })
  assert.deepEqual(expectedPaneFocus(before, "close"), { keys: ["one", "three"] })
  assert.equal(
    expectedPaneFocus({ ...before, focusedPane: "one" }, "previous").target,
    "one",
  )
  assert.equal(
    expectedPaneFocus({ ...before, focusedPane: "three" }, "next").target,
    "three",
  )
  assert.throws(() => expectedPaneFocus(before, 9), CannotRun)
  for (const move of ["new", "close", "next", "previous", "same", 0])
    assert.throws(
      () => expectedPaneFocus({ ...before, focusedPane: null }, move),
      CannotRun,
    )
})

test("retirement requires work but does not assume one frame owner", () => {
  for (const count of [1, 5, 17]) assert.doesNotThrow(() => requirePendingMovement(count))
  for (const count of [0, -1, 1.5, undefined])
    assert.throws(() => requirePendingMovement(count), CannotRun)
})

function pageFixture(t, expected, wait) {
  const dom = fixture(t, expected)
  const disposed = []
  const page = {
    async evaluateHandle(create, input) {
      assert.deepEqual(input, { selectors: css, expected })
      const owner = create(input)
      return {
        value: owner,
        evaluate: (read) => read(owner),
        async dispose() {
          disposed.push("owner")
        },
      }
    },
    async waitForFunction(predicate, handle, options) {
      await wait({ dom, ready: () => predicate(handle.value), options })
      return {
        async dispose() {
          disposed.push("ready")
        },
      }
    },
  }
  return { page, disposed }
}

test("the bounded page wait accepts delayed convergence and releases both handles", async (t) => {
  const expected = { keys: ["old"], added: true }
  const { page, disposed } = pageFixture(t, expected, ({ dom, ready, options }) => {
    assert.deepEqual(options, { timeout: 3000, polling: "raf" })
    for (let frame = 0; frame < 6; frame++) assert.equal(ready(), false)
    dom.paint({ keys: ["old", "new"], marked: ["new"], caret: "new" })
    assert.equal(ready(), true)
  })
  const report = await paneFocusSettled(page, expected)
  assert.equal(report.converged, true)
  assert.equal(report.trace.length, 7)
  assert.deepEqual(disposed, ["ready", "owner"])
})

for (const late of [false, true]) {
  test(`deadline failure stays failed with ${late ? "late" : "missing"} convergence`, async (t) => {
    const expected = { keys: ["old"], added: true }
    const { page, disposed } = pageFixture(t, expected, ({ dom, ready, options }) => {
      assert.equal(options.timeout, 23)
      assert.equal(ready(), false)
      if (late) dom.paint({ keys: ["old", "new"], marked: ["new"], caret: "new" })
      throw new errors.TimeoutError("fixture exhausted its budget")
    })
    const report = await paneFocusSettled(page, expected, 23)
    assert.equal(report.converged, false)
    assert.equal(report.after.activeInPane, late ? "new" : "old")
    assert.deepEqual(disposed, ["owner"])
  })
}

test("a lost page is an infrastructure failure, not a settlement timeout", async (t) => {
  const expected = { keys: ["old"], target: "old" }
  const failure = new Error("page closed")
  const { page, disposed } = pageFixture(t, expected, () => {
    throw failure
  })
  await assert.rejects(paneFocusSettled(page, expected), (error) => error === failure)
  assert.deepEqual(disposed, ["owner"])
})
