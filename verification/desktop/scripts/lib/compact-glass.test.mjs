import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { tsImport } from "tsx/esm/api"
import { CannotRun } from "./cli.mjs"
import { css, keys, storage, zoneSaid } from "./selectors.mjs"
import { residueFailures } from "./workspace.mjs"

const { zonesAt, settlingSpeed } = await tsImport(
  "../../../../src/desktop/split-panes/model/drop.ts",
  import.meta.url,
)
const source = readFileSync(new URL("../drag.mjs", import.meta.url), "utf8")
const begin = source.indexOf('  "compact-glass-release": async ')
const end = source.indexOf('  "compact-drag-card":', begin)
assert.ok(begin >= 0 && end > begin)
const method = source.slice(begin, end).trim().replace(/,$/, "")

// Pair the observed 1/64px DOM rectangle with a reachable model box at
// x384, width300. This reproduces the boundary without claiming the Mac
// prepared state was captured. Approaching from the left can retain Left.
const middle = { key: "2", x: 383.984375, y: 8, w: 299.984375, h: 684 }
const prepared = { x: 384, y: 8, w: 300, h: 684 }
const before = ["1", "2", "4"]
const list = [
  { key: "1", x: 8, y: 8, w: 367.984375, h: 684 },
  middle,
  { key: "4", x: 692, y: 8, w: 299.984375, h: 684 },
]
function offered(x, y, speed, prior = "left") {
  return zonesAt(
    {
      x: x - prepared.x,
      y: y - prepared.y,
      velocity: { x: speed * 0.8, y: speed * 0.6 },
      travel: { x: 400, y: 300 },
    },
    { width: prepared.w, height: prepared.h },
    prior,
  ).zone
}

test("a drawn midpoint can retain Left in the unchanged fractional prior-side model", () => {
  assert.equal(settlingSpeed, 0.25)
  const x = middle.x + middle.w / 2
  const y = middle.y + middle.h / 2
  assert.equal(x, 533.9765625)
  assert.equal(offered(x, y, 0.2385), "left")
  assert.equal(offered(x, y, 0.344), "center")
})

function fixture({
  speed = 0.2385,
  prior = "left",
  initial = [],
  writes = [],
  wrongOrder = false,
  committed = true,
  residue = {},
  absent = false,
} = {}) {
  const pages = []
  const fresh = async ({ prefs }) => {
    const page = {
      motion: prefs[storage.motion],
      cancelled: false,
      live: before.slice(),
      mouse: {
        async move(x, y, options) {
          assert.deepEqual(JSON.parse(JSON.stringify(options)), { steps: 20 })
          page.point = { x, y }
        },
        async up() {
          page.live = page.cancelled || wrongOrder ? before.slice() : ["2", "1", "4"]
        },
      },
      keyboard: {
        async press(key) {
          assert.equal(key, keys.escape)
          page.cancelled = true
        },
      },
      async evaluateHandle() {
        return {
          evaluate: async (fn) => fn({ stop: () => ({ initial, writes }) }),
          async dispose() {},
        }
      },
      async waitForSelector(selector, options) {
        assert.equal(selector, css.dragGhost)
        assert.deepEqual(JSON.parse(JSON.stringify(options)), {
          state: "hidden",
          timeout: 3000,
        })
      },
    }
    pages.push(page)
    return page
  }
  const check = runInNewContext(`({${method}})["compact-glass-release"]`, {
    storage,
    marks: { pressing: "data-drag-pressing", blur: "data-drag-blur" },
    css,
    keys,
    zoneSaid,
    CannotRun,
    residueFailures,
    threeColumns: async () => list,
    recordShapes: async () => ({ async stop() {} }),
    lift: async () => {},
    zoneSays: async (page, pattern) => {
      assert.equal(pattern, zoneSaid.swap)
      return !absent && offered(page.point.x, page.point.y, speed, prior) === "center"
    },
    recordedDropSettled: async (page, expected, ...extra) => {
      assert.deepEqual(extra, [], "the owning settlement budget is unchanged")
      assert.deepEqual(Array.from(expected), page.cancelled ? before : ["2", "1", "4"])
      return committed
    },
    frames: async (_page, count) => assert.equal(count, 9),
    order: async (page) => page.live,
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
  })
  return { run: () => check(null, "columns", fresh), pages }
}

for (const speed of [0, 0.2385, 0.344]) {
  test(`the owning compact check targets inside Swap when approaching at ${speed}px/ms`, async () => {
    const f = fixture({ speed })
    const result = await f.run()
    assert.deepEqual(Array.from(result.failures), [])
    assert.equal(result.cases.length, 4)
    assert.deepEqual(
      f.pages.map((p) => p.motion),
      ["full", "full", "reduced", "reduced"],
    )
    assert.deepEqual(
      f.pages.map((p) => p.cancelled),
      [true, false, true, false],
    )
    // The selected input has room on both sides under the same prior state;
    // an exact rounded boundary point is not an interior-point regression.
    for (const { point } of f.pages)
      for (const dx of [-0.5, 0, 0.5])
        assert.equal(offered(point.x + dx, point.y, speed), "center")
  })
}

for (const [name, config] of [
  ["initial glass", { initial: ["data-drag-blur"] }],
  ["a glass pulse", { writes: [{ held: true, name: "data-drag-pressing" }] }],
  ["wrong committed order", { wrongOrder: true }],
  ["an unsettled commit", { committed: false }],
  ["a remaining copy", { residue: { copies: 1 } }],
]) {
  test(`the owning compact check still rejects ${name}`, async () => {
    const result = await fixture(config).run()
    assert.ok(result.failures.length)
  })
}

test("an absent offer still leaves the compact case uncredited", async () => {
  await assert.rejects(fixture({ absent: true }).run, /no swap was offered/)
})

test("opposite prior-side ownership is not reclassified as Swap", async () => {
  assert.equal(offered(middle.x + middle.w / 2 + 1, 350, 0, "right"), "right")
  await assert.rejects(fixture({ speed: 0, prior: "right" }).run, /no swap was offered/)
})
