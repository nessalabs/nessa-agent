import { test } from "node:test"
import assert from "node:assert/strict"
import { runInNewContext } from "node:vm"
import { CannotRun } from "./cli.mjs"
import { keysWhileRowsArrive, pressMountedOverviewKeys } from "./overview-keyboard.mjs"

/**
 * Regression of c13a9ed66f (#606): observation and action were separate RPCs.
 * The page completes its next render before the opening-key RPC returns.
 */
function pageWithFastCompletion({
  noPartial = false,
  noActiveFocus = false,
  outsideFocus = false,
  deliverBeforeOpeningFailure = false,
  openingFailure,
} = {}) {
  const events = []
  const view = {
    visible: false,
    complete: false,
    rows: [],
    focused: undefined,
    listedChecks: [],
    observerId: undefined,
  }
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
  const column = {
    hasAttribute(name) {
      view.listedChecks.push(name)
      return view.complete && name === "data-overview-listed"
    },
    querySelectorAll: () => view.rows,
  }
  const document = {
    documentElement: {
      getAttribute(name) {
        return name === "data-overview-key-observer" ? view.observerId : null
      },
      setAttribute(name, value) {
        if (name === "data-overview-key-observer") view.observerId = value
      },
      removeAttribute(name) {
        if (name === "data-overview-key-observer") view.observerId = undefined
      },
    },
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
  let timeout
  let settled = false
  let disposed = false
  const page = {
    waitForFunction(predicate, selectors, options) {
      events.push("arm")
      assert.equal(options.timeout, 2000)
      assert.equal(options.polling, "raf")
      return new Promise((resolve, reject) => {
        timeout = reject
        observe = () => {
          const result = browser(predicate, selectors)
          if (!result) return
          settled = true
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
    evaluate: async (operation, value) =>
      runInNewContext(`(${operation.toString()})(value)`, { document, value }),
    keyboard: {
      press: async () => {
        events.push("open")
        if (openingFailure) {
          if (deliverBeforeOpeningFailure) {
            view.visible = true
            events.push("delivered")
          }
          throw openingFailure
        }
        view.visible = true
        view.complete = noPartial
        if (noActiveFocus) view.focused = undefined
        if (outsideFocus) view.focused = new Row("outside")
        observe?.()
        view.complete = true
        view.rows.push(new Row("third"))
        events.push("complete")
        if (!settled) timeout?.(new Error("page.waitForFunction timed out"))
      },
    },
  }
  return {
    page,
    events,
    view,
    poll: () => observe?.(),
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
  assert.equal(scene.view.observerId, undefined)
  assert.equal(scene.disposed, true)
})

test("reports a genuinely absent partial prefix without inventing measurements", async () => {
  const scene = pageWithFastCompletion({ noPartial: true })
  await assert.rejects(keysWhileRowsArrive(scene.page), CannotRun)
  assert.deepEqual(scene.events, ["arm", "open", "complete"])
  assert.equal(scene.disposed, true)
})

test("reports a completed list when no row is focused", async () => {
  const scene = pageWithFastCompletion({ noPartial: true, noActiveFocus: true })
  await assert.rejects(keysWhileRowsArrive(scene.page), CannotRun)
  assert.deepEqual(scene.events, ["arm", "open", "complete"])
  assert.deepEqual(scene.view.listedChecks, ["data-overview-listed"])
  assert.equal(scene.disposed, true)
})

test("reports a completed list when focus is outside its column", async () => {
  const scene = pageWithFastCompletion({ noPartial: true, outsideFocus: true })
  await assert.rejects(keysWhileRowsArrive(scene.page), CannotRun)
  assert.deepEqual(scene.events, ["arm", "open", "complete"])
  assert.deepEqual(scene.view.listedChecks, ["data-overview-listed"])
  assert.equal(scene.disposed, true)
})

test("preserves an opening-key failure", async () => {
  const failure = new Error("opening key failed")
  const scene = pageWithFastCompletion({ openingFailure: failure })
  await assert.rejects(keysWhileRowsArrive(scene.page), (error) => error === failure)
  assert.deepEqual(scene.events, ["arm", "open"])
})

test("stops observing when an opening key is delivered but its RPC rejects", async () => {
  const failure = new Error("opening key acknowledgement failed")
  const scene = pageWithFastCompletion({
    deliverBeforeOpeningFailure: true,
    openingFailure: failure,
  })
  await assert.rejects(keysWhileRowsArrive(scene.page), (error) => error === failure)
  scene.poll()
  assert.deepEqual(scene.events, ["arm", "open", "delivered"])
  assert.deepEqual(scene.view.listedChecks, [])
  assert.equal(scene.view.observerId, undefined)
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
