import { test } from "node:test"
import assert from "node:assert/strict"
import { runInNewContext } from "node:vm"
import { CannotRun } from "./cli.mjs"
import { keysWhileRowsArrive, pressMountedOverviewKeys } from "./overview-keyboard.mjs"

/**
 * Regression of c13a9ed66f (#606): observation and action were separate RPCs.
 * The page completes its next render before the opening-key RPC returns.
 */
function pageWithFastCompletion({ noPartial = false, openingFailure } = {}) {
  const events = []
  const view = { visible: false, complete: false, rows: [], focused: undefined }
  class Row {
    constructor(id) {
      this.id = id
    }
    get tabIndex() {
      return view.focused === this ? 0 : -1
    }
    closest() {
      return this
    }
    getAttribute() {
      return this.id
    }
    dispatchEvent(event) {
      events.push(event.code)
      if (event.code === "End" || event.code === "ArrowDown")
        view.focused = view.rows.at(-1)
      if (event.code === "Home") view.focused = view.rows[0]
    }
  }
  view.rows = [new Row("first"), new Row("second")]
  view.focused = view.rows[0]
  const column = { hasAttribute: () => view.complete, querySelectorAll: () => view.rows }
  const document = {
    querySelector: () => (view.visible ? column : null),
    get activeElement() {
      return view.focused
    },
  }
  const browser = (predicate, selectors) =>
    runInNewContext(`(${predicate.toString()})(selectors)`, {
      document,
      selectors,
      HTMLElement: Row,
      KeyboardEvent: class {
        constructor(type, fields) {
          Object.assign(this, fields)
        }
      },
    })
  let observe
  let disposed = false
  const page = {
    waitForFunction(predicate, selectors, options) {
      events.push("arm")
      assert.equal(options.timeout, 2000)
      assert.equal(options.polling, "raf")
      return new Promise((resolve) => {
        observe = () => {
          const result = browser(predicate, selectors)
          if (!result) return
          resolve({
            jsonValue: async () => JSON.parse(JSON.stringify(result)),
            dispose: async () => {
              disposed = true
            },
          })
        }
        if (view.visible) observe()
      })
    },
    evaluate: async (predicate, selectors) => browser(predicate, selectors),
    keyboard: {
      press: async () => {
        events.push("open")
        if (openingFailure) throw openingFailure
        view.visible = true
        view.complete = noPartial
        observe?.()
        view.complete = true
        view.rows.push(new Row("third"))
        events.push("complete")
      },
    },
  }
  return {
    page,
    events,
    view,
    get disposed() {
      return disposed
    },
  }
}

test("acts on the observed partial prefix before a later render completes", async () => {
  const scene = pageWithFastCompletion()
  const result = await keysWhileRowsArrive(scene.page)
  assert.deepEqual(scene.events, ["arm", "open", "End", "ArrowDown", "Home", "complete"])
  assert.deepEqual(result, {
    missed: false,
    first: "first",
    last: "second",
    afterEnd: { focused: "second", tabbable: "second" },
    afterDown: { focused: "second", tabbable: "second" },
    afterHome: { focused: "first", tabbable: "first" },
    failures: [],
  })
  assert.equal(scene.view.rows.length, 3)
  assert.equal(scene.disposed, true)
})

test("reports a genuinely absent partial prefix without inventing measurements", async () => {
  const scene = pageWithFastCompletion({ noPartial: true })
  await assert.rejects(keysWhileRowsArrive(scene.page), CannotRun)
  assert.deepEqual(scene.events, ["arm", "open", "complete"])
  assert.equal(scene.disposed, true)
})

test("preserves an opening-key failure", async () => {
  const failure = new Error("opening key failed")
  const scene = pageWithFastCompletion({ openingFailure: failure })
  await assert.rejects(keysWhileRowsArrive(scene.page), (error) => error === failure)
  assert.deepEqual(scene.events, ["arm", "open"])
})

test("does not act before its own rows and focus are mounted", () => {
  const scene = pageWithFastCompletion()
  // The same serialized callback is used by real Playwright pages.
  const result = runInNewContext(
    `(${pressMountedOverviewKeys.toString()})({column:"column",item:"row"})`,
    {
      document: { querySelector: () => null },
    },
  )
  assert.equal(result, false)
  assert.deepEqual(scene.events, [])
})

test("preserves a browser observation fault instead of reporting unavailable rendering", async () => {
  const failure = new Error("browser observation failed")
  const scene = pageWithFastCompletion()
  scene.page.waitForFunction = () => Promise.reject(failure)
  await assert.rejects(keysWhileRowsArrive(scene.page), (error) => error === failure)
})
