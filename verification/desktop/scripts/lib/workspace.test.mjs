import assert from "node:assert/strict"
import { it } from "node:test"
import { runInNewContext } from "node:vm"
import { JSDOM } from "jsdom"
import { css, zoneSaid } from "./selectors.mjs"
import {
  overviewCompleteList,
  overviewRowReady,
  paneDropFailure,
  recordZones,
  sweepZoneEvidence,
} from "./workspace.mjs"

it("accepts a moved pane after the carried copy is gone", () => {
  assert.equal(paneDropFailure({ order: "1,2" }, { order: "2,1", ghost: 0 }), null)
})
it("rejects a no-op even when no carried copy remains", () => {
  assert.equal(
    paneDropFailure({ order: "1,2" }, { order: "1,2", ghost: 0 }),
    "the drop did not change pane order",
  )
})
it("rejects a moved pane while its carried copy remains", () => {
  assert.equal(
    paneDropFailure({ order: "1,2" }, { order: "2,1", ghost: 1 }),
    "the carried copy is still on the page after the drop",
  )
})

function overview(t, { rows = 2, focused = 0, listed = false, outside = false } = {}) {
  const dom =
    new JSDOM(`<main><div class="agents-overview-column" ${listed ? "data-overview-listed" : ""}>
    ${Array.from({ length: rows }, (_, i) => `<button data-overview-item="${i}"></button>`).join("")}
    </div><button id="outside" data-overview-item="other"></button></main>`)
  const previous = globalThis.document
  globalThis.document = dom.window.document
  t.after(() => {
    globalThis.document = previous
    dom.window.close()
  })
  const items = document.querySelectorAll(`${css.overviewColumn} ${css.overviewItem}`)
  if (outside) document.getElementById("outside").focus()
  else items[focused]?.focus()
  const budgets = []
  return {
    budgets,
    page: {
      async waitForFunction(predicate, arg, options) {
        budgets.push(options)
        if (!predicate(arg)) throw new Error("fixture prerequisite remained unavailable")
      },
    },
  }
}

it("focus and Down are ready on a mounted prefix while complete-list timing still fails", async (t) => {
  const { page, budgets } = overview(t)
  assert.equal(await overviewRowReady(page, true), true)
  const complete = await overviewCompleteList(page)
  assert.equal(complete.complete, false)
  assert.equal(complete.failures.length, 1)
  assert.equal(complete.budgetMs, 2000)
  assert.deepEqual(
    budgets.map((o) => o.timeout),
    [2000, 2000],
  )
  assert.ok(budgets.every((o) => o.polling === "raf"))
})
it("a complete list does not excuse lost focus", async (t) => {
  const { page } = overview(t, { focused: -1, listed: true })
  assert.equal(await overviewRowReady(page), false)
  assert.equal((await overviewCompleteList(page)).complete, true)
})
it("a row outside the overview column does not certify focus", async (t) => {
  const { page } = overview(t, { outside: true })
  assert.equal(await overviewRowReady(page), false)
})
it("Down waits for its mounted successor rather than accepting the last prefix row", async (t) => {
  const { page } = overview(t, { rows: 1 })
  assert.equal(await overviewRowReady(page), true)
  assert.equal(await overviewRowReady(page, true), false)
})
it("a stalled empty prefix fails readiness", async (t) => {
  const { page } = overview(t, { rows: 0 })
  assert.equal(await overviewRowReady(page, true), false)
})

const entry = (at, delivered, said = "Swap with fixture", stamp = 100) => ({
  said,
  at,
  lastMoveAt: stamp,
  lastMoveDeliveredAt: delivered,
})
const sweep = (records) => sweepZoneEvidence(records, 150, zoneSaid.vertical)
it("a timely horizontal announcement holds and a moving vertical announcement fails", () => {
  assert.deepEqual(sweep([entry(120, 101)]).failures, [])
  assert.equal(sweep([entry(120, 101, "Move above fixture")]).failures.length, 1)
})
it("absent announcements and announcements without a recorded move fail", () => {
  assert.equal(sweep([]).failures.length, 1)
  assert.equal(
    sweep([{ said: "Swap", at: 1, lastMoveAt: null, lastMoveDeliveredAt: null }]).moving
      .length,
    0,
  )
  assert.equal(
    sweep([{ said: "Swap", at: 1, lastMoveAt: null, lastMoveDeliveredAt: null }]).failures
      .length,
    1,
  )
})
it("late delivery is distinct from late observation and neither proves moving intent", () => {
  const delivery = sweep([entry(274, 270)])
  const observation = sweep([entry(274, 100)])
  assert.equal(delivery.timing[0].deliveryLagMs, 170)
  assert.equal(delivery.timing[0].observationLagMs, 4)
  assert.equal(observation.timing[0].deliveryLagMs, 0)
  assert.equal(observation.timing[0].observationLagMs, 174)
  assert.equal(delivery.failures.length, 1)
  assert.equal(observation.failures.length, 1)
})
it("the exact rest boundary and future event clocks cannot certify moving intent", () => {
  assert.equal(sweep([entry(249, 101)]).failures.length, 0)
  assert.equal(sweep([entry(250, 101)]).failures.length, 1)
  assert.equal(sweep([entry(110, 90)]).failures.length, 1)
})
it("a later legitimate rest announcement is retained without replacing moving evidence", () => {
  const result = sweep([entry(120, 101), entry(270, 101, "Move above fixture")])
  assert.equal(result.timing.length, 2)
  assert.equal(result.moving.length, 1)
  assert.equal(result.failures.length, 0)
})

it("the browser recorder captures delivery separately, ignores duplicate coordinates and releases resources", async () => {
  let now = 0,
    moved,
    notify,
    disposed = false,
    disconnected = false
  const status = { textContent: "" }
  const page = {
    async evaluateHandle(fn, arg) {
      const value = runInNewContext(`(${fn.toString()})(arg)`, {
        arg,
        document: {
          querySelector: () => status,
          addEventListener: (_event, callback) => {
            moved = callback
          },
          removeEventListener: () => {
            moved = null
          },
        },
        performance: { now: () => now },
        MutationObserver: class {
          constructor(callback) {
            notify = callback
          }
          observe() {}
          disconnect() {
            disconnected = true
          }
        },
      })
      return {
        evaluate: async (fn) => fn(value),
        dispose: async () => {
          disposed = true
        },
      }
    },
  }
  const recorder = await recordZones(page)
  now = 1
  status.textContent = "Swap with fixture"
  notify()
  now = 270
  moved({ clientX: 10, clientY: 30, timeStamp: 100 })
  now = 271
  moved({ clientX: 10, clientY: 30, timeStamp: 271 })
  now = 274
  status.textContent = "Swap with fixture"
  notify()
  const recorded = await recorder.take()
  assert.deepEqual(JSON.parse(JSON.stringify(recorded)), [
    { said: "Swap with fixture", at: 1, lastMoveAt: null, lastMoveDeliveredAt: null },
    entry(274, 270),
  ])
  assert.equal(sweep(recorded).failures.length, 1)
  assert.equal(disposed, true)
  assert.equal(disconnected, true)
  assert.equal(moved, null)
})
