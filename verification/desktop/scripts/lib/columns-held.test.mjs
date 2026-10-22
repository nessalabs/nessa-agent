import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { CannotRun } from "./cli.mjs"
import { css, keys } from "./selectors.mjs"
import { hideColumns, settled } from "./workspace.mjs"

// Run the owning CLI check; retain its actual sampling and failure assertions.
const source = readFileSync(new URL("../columns.mjs", import.meta.url), "utf8")
const begin = source.indexOf('  "draft-held": async ')
const end = source.indexOf("\n  /**", begin)
assert.ok(begin >= 0 && end > begin)
const method = source.slice(begin, end).trim().replace(/,$/, "")

async function check(layout, damage = {}) {
  let listOpen = layout === "columns"
  let sidebarOpen = true
  let count = 1
  let text = ""
  let onKey
  const events = []
  const window = {
    addEventListener: (_name, callback) => {
      onKey = callback
    },
  }
  const document = {
    getAnimations: () => [],
    querySelectorAll: () => {
      const sample = [
        { label: "Original", home: false, text: "" },
        { label: "New session", home: true, text, ...damage },
      ]
      if (damage.missing) sample.pop()
      return sample.map((pane) => ({
        getAttribute: () => pane.label,
        querySelector: (selector) =>
          selector === css.paneHome ? (pane.home ? {} : null) : { value: pane.text },
      }))
    },
  }
  const evaluate = (fn, argument) =>
    runInNewContext(`(${fn.toString()})(argument)`, {
      window,
      document,
      argument,
      requestAnimationFrame: (callback) => callback(),
    })
  const page = {
    keyboard: {
      async press(key) {
        events.push(key)
        if (key === keys.toggleSidebar) sidebarOpen = !sidebarOpen
        if (key === keys.toggleSessionList) listOpen = !listOpen
        if (key === keys.newSessionBeside && !listOpen && !sidebarOpen) count++
        if (key === keys.closePane) {
          onKey({ code: "KeyW", metaKey: true })
          count = damage.after ?? 1
        }
      },
    },
    locator: () => ({
      count: async () => count,
      fill: async (value) => {
        text = value
      },
      inputValue: async () => text,
    }),
    evaluate: async (fn, argument) => evaluate(fn, argument),
    waitForFunction: async (fn, argument) => assert.ok(evaluate(fn, argument)),
  }
  const body = runInNewContext(`({${method}})["draft-held"]`, {
    css,
    keys,
    CannotRun,
    hideColumns,
    settled,
    frames: async () => {},
    need: async () => {
      if (count !== 2)
        throw new CannotRun("the split was refused: side columns left no room")
    },
  })
  const result = await body({ page, layout })
  return { result, events }
}

for (const layout of ["columns", "sidebar"]) {
  test(`draft close check makes room before splitting in ${layout}`, async () => {
    const { result, events } = await check(layout)
    assert.deepEqual(Array.from(result.failures), [])
    assert.equal(result.before, 1)
    assert.equal(result.after, 1)
    assert.ok(events.indexOf(keys.toggleSidebar) < events.indexOf(keys.newSessionBeside))
    if (layout === "columns")
      assert.ok(
        events.indexOf(keys.toggleSessionList) < events.indexOf(keys.newSessionBeside),
      )
    else assert.ok(!events.includes(keys.toggleSessionList))
  })
}
for (const [name, damage, part] of [
  ["missing draft", { missing: true }, "no unsent text"],
  ["dropped text", { text: "" }, "no unsent text"],
  ["missing home", { home: false }, "not the home"],
  ["changed label", { label: "Missing" }, "labelled"],
  ["retained box", { after: 2 }, "after the box goes"],
]) {
  test(`draft close still rejects ${name}`, async () => {
    const { result } = await check("columns", damage)
    assert.ok(
      result.failures.some((failure) => failure.includes(part)),
      JSON.stringify(result),
    )
  })
}
