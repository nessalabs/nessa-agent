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

async function check(layout, damage = {}, { width = 760, rail = 84 } = {}) {
  let listOpen = layout === "columns"
  // The narrow columns layout starts with a chosen-open but folded sidebar.
  let sidebar = { open: true, folded: layout === "columns" }
  const drawn = () => sidebar.open && !sidebar.folded
  const room = () => ({
    width: width - rail - 16 - (listOpen ? 268 : 0) - (drawn() ? 208 : 0),
    spare: drawn() ? 208 : 0,
  })
  let beforeSplit
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
        if (key === keys.toggleSidebar) sidebar = { open: !drawn(), folded: false }
        if (key === keys.toggleSessionList) listOpen = !listOpen
        if (key === keys.newSessionBeside) {
          const measured = room()
          beforeSplit = { ...measured, sidebarDrawn: drawn() }
          // Two readable 300px panes plus their 8px gutter. The real store
          // probe covers this admission and its use of measured sidebar spare.
          if (measured.width + measured.spare >= 608) {
            count++
            if (measured.width < 608) sidebar = { ...sidebar, folded: true }
          }
        }
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
  return { result, events, beforeSplit, sidebar }
}

for (const layout of ["columns", "sidebar"]) {
  test(`draft close check makes room before splitting in ${layout}`, async () => {
    const { result, events, beforeSplit, sidebar } = await check(layout)
    assert.deepEqual(Array.from(result.failures), [])
    assert.equal(result.before, 1)
    assert.equal(result.after, 1)
    if (layout === "columns") {
      assert.equal(beforeSplit.sidebarDrawn, true)
      assert.ok(beforeSplit.width < 608)
      assert.ok(beforeSplit.width + beforeSplit.spare >= 608)
      assert.equal(sidebar.folded, true)
    } else {
      assert.equal(beforeSplit.sidebarDrawn, false)
      assert.equal(beforeSplit.spare, 0)
    }
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

test("draft setup still refuses when even reclaimed sidebar room is insufficient", async () => {
  await assert.rejects(() => check("columns", {}, { width: 707 }), CannotRun)
})

test("draft setup admits the exact readable-room boundary using sidebar spare", async () => {
  const { result, beforeSplit, sidebar } = await check("columns", {}, { width: 708 })
  assert.deepEqual(Array.from(result.failures), [])
  assert.equal(beforeSplit.width + beforeSplit.spare, 608)
  assert.equal(sidebar.folded, true)
})
