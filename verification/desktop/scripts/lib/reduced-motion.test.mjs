import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { errors } from "playwright"
import { CannotRun } from "./cli.mjs"
import { css, storage, zoneSaid } from "./selectors.mjs"
import { recordedDropReady, recordedDropSettled } from "./shape-sampler.mjs"
import { residueFailures } from "./workspace.mjs"

// Exercise the actual CLI check without launching its main or copying its rule.
const source = readFileSync(new URL("../drag.mjs", import.meta.url), "utf8")
const begin = source.indexOf('  "reduced-motion": async ')
const end = source.indexOf('  "compact-drag-card":', begin)
assert.ok(begin >= 0 && end > begin, "the owning check must be found")
const method = source.slice(begin, end).trim().replace(/,$/, "")

function fixture({
  commitAt = 6,
  lastFrame = 8,
  late = false,
  animate = false,
  missing = false,
  wrong = false,
  extra = false,
  failure,
  placeholders = 1,
  glided = false,
  jumped = false,
  residue = {},
} = {}) {
  const before = ["1", "2", "4"]
  const expected = ["2", "1", "4"]
  const list = before.map((key, index) => ({ key, x: index * 100, y: 0, w: 100, h: 200 }))
  const final = list.map((pane) => ({ ...pane, x: expected.indexOf(pane.key) * 100 }))
  const window = { __shapes: [] }
  const document = {
    getAnimations: () =>
      animate
        ? [
            {
              playState: animate === "pending" ? "pending" : "running",
              effect: {
                getTiming: () => ({ iterations: animate === "infinite" ? Infinity : 1 }),
              },
            },
          ]
        : [],
  }
  const landed = []
  const events = []
  let live = before
  const page = {
    mouse: {
      async move() {},
      async up() {
        events.push("release")
      },
    },
    locator: () => ({ count: async () => placeholders }),
    async waitForTimeout() {},
    async waitForFunction(predicate, argument, options) {
      assert.deepEqual(options, { timeout: 3000, polling: "raf" })
      assert.deepEqual(Array.from(argument), expected)
      if (failure) throw failure
      for (let frame = 1; frame <= lastFrame; frame++) {
        live = frame >= commitAt ? expected : before
        let panes = frame >= commitAt ? final : list
        if (wrong) panes = list
        if (extra) panes = [...panes, { key: "extra", x: 400, y: 0, w: 100, h: 200 }]
        if (missing) panes = panes.slice(0, 2)
        if (jumped && frame === 3)
          panes = panes.map((pane) => ({ ...pane, x: pane.x + 40 }))
        landed.push({ t: frame * 16, panes })
        window.__shapes = landed
        events.push(`frame-${frame}`)
        if (
          runInNewContext(`(${predicate.toString()})(argument)`, {
            window,
            document,
            argument,
          })
        )
          return {
            async dispose() {
              events.push("dispose-ready")
            },
          }
      }
      if (late) live = expected
      throw new errors.TimeoutError("controlled observation deadline")
    },
  }
  let recordings = 0
  const check = runInNewContext(`({${method}})["reduced-motion"]`, {
    storage,
    css,
    zoneSaid,
    CannotRun,
    recordedDropSettled,
    residueFailures,
    threeColumns: async () => list,
    recordShapes: async () => {
      const landing = recordings++ > 0
      return {
        stop: async () => {
          events.push(landing ? "landing-stop" : "preview-stop")
          return landing
            ? landed
            : [
                { zone: "swap", panes: list },
                {
                  zone: "swap",
                  panes: glided
                    ? list.map((pane) => ({ ...pane, x: pane.x + 10 }))
                    : list,
                },
              ]
        },
      }
    },
    lift: async () => {},
    zoneSays: async () => true,
    order: async () => live,
    dragResidue: async () => ({
      copies: 0,
      placeholders: 0,
      shields: 0,
      lifted: 0,
      dragging: 0,
      transformed: [],
      sized: [],
      clipped: [],
      ...residue,
    }),
    near: (a, b) => Math.abs(a - b) <= 2,
    px: (pane) => JSON.stringify(pane),
  })
  return { run: () => check(page, "sidebar", async () => page), events, landed }
}

test("the owning check retains all six landing frames through the expected swap", async () => {
  const { run, events, landed } = fixture()
  const result = await run()
  assert.equal(result.committed, true)
  assert.deepEqual(Array.from(result.failures), [])
  assert.equal(landed.length, 6)
  assert.ok(events.indexOf("landing-stop") > events.indexOf("frame-6"))
  assert.equal(events.filter((event) => event === "dispose-ready").length, 1)
})

for (const [name, config] of [
  ["stalled original order", { commitAt: Infinity }],
  ["missing pane", { missing: true }],
  ["wrong complete order", { wrong: true }],
  ["an extra pane", { extra: true }],
  ["unfinished finite animation", { animate: true }],
  ["pending finite animation", { animate: "pending" }],
  ["late live order after the deadline", { commitAt: Infinity, late: true }],
]) {
  test(`the owning check rejects ${name}`, async () => {
    const result = await fixture(config).run()
    assert.equal(result.committed, false)
    assert.ok(result.failures.some((line) => line.includes("within its budget")))
  })
}

test("infinite ambient animation does not prevent a completed reduced-motion drop", async () => {
  const result = await fixture({ animate: "infinite" }).run()
  assert.equal(result.committed, true)
  assert.deepEqual(Array.from(result.failures), [])
})

test("recorded reading order is columns then rows, independent of DOM order", () => {
  const window = {
    __shapes: [
      {
        panes: [
          { key: "lower", x: 0, y: 100 },
          { key: "right", x: 100, y: 0 },
          { key: "upper", x: 0, y: 0 },
        ],
      },
    ],
  }
  const document = { getAnimations: () => [] }
  assert.equal(
    runInNewContext(`(${recordedDropReady.toString()})(["upper","lower","right"])`, {
      window,
      document,
    }),
    true,
  )
  assert.equal(
    runInNewContext(`(${recordedDropReady.toString()})(["upper","right","lower"])`, {
      window,
      document,
    }),
    false,
  )
  assert.deepEqual(
    window.__shapes[0].panes.map((pane) => pane.key),
    ["lower", "right", "upper"],
  )
})

test("no recorded landing frame is unfinished, rather than ready or an exception", () => {
  const window = { __shapes: [] }
  assert.equal(
    runInNewContext(`(${recordedDropReady.toString()})(["1","2","4"])`, { window }),
    false,
  )
})

test("a page failure propagates instead of being labelled settlement timeout", async () => {
  const failure = new Error("page closed")
  await assert.rejects(fixture({ failure }).run(), (error) => error === failure)
})

for (const [name, config, failure] of [
  ["no placeholder", { placeholders: 0 }, "no placeholder"],
  ["preview glide", { glided: true }, "a pane glided"],
  ["intermediate landing geometry", { jumped: true }, "drawn off where it landed"],
  ["a carried copy", { residue: { copies: 1 } }, "copies left"],
  ["an inline transform", { residue: { transformed: ["1"] } }, "drawn off their place"],
]) {
  test(`converged order still rejects ${name}`, async () => {
    const result = await fixture(config).run()
    assert.equal(result.committed, true)
    assert.ok(result.failures.some((line) => line.includes(failure)))
  })
}
