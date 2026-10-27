import assert from "node:assert/strict"
import { test } from "node:test"
import { runInNewContext } from "node:vm"

import { css, selectorFor } from "./selectors.mjs"
import {
  openStressConversation,
  selectedStressConversation,
} from "./stress-conversation.mjs"

const title = "stress reply r1 aside s1"
const earlier = "2b3c7de2-d04a-45b6-a0bb-77cae7adb600"
const intended = "6f23287f-c023-4249-a006-b3da7824633d"

/** The page seam retains each row's identity and the composer it opens. */
function windowWith(
  ids,
  {
    deferSelection = false,
    composerVisible = true,
    composerCount = 1,
    composerWidth = 200,
    composerHeight = 40,
  } = {},
) {
  const rows = ids.map((id) => ({ id, title }))
  let active = deferSelection ? earlier : undefined
  let publish
  const clicks = []
  const waits = []
  const page = {
    locator(selector, options) {
      const selected = rows.filter(
        (row) =>
          selector === selectorFor.sessionListRowFor(row.id) &&
          (!options?.hasText || row.title.includes(options.hasText)),
      )
      return {
        async click(options) {
          assert.equal(selected.length, 1, "the requested row must be unique")
          if (!deferSelection) active = selected[0].id
          clicks.push({ id: selected[0].id, ...options })
        },
      }
    },
    async waitForFunction(read, argument, options) {
      waits.push({ id: active, ...options })
      const sample = () =>
        runInNewContext(`(${read.toString()})`, {
          document: {
            querySelectorAll(selector) {
              if (selector === css.selectedSessionListRow)
                return active ? [{ getAttribute: () => active }] : []
              assert.equal(selector, `${css.conversationDock} textarea`)
              return Array.from({ length: composerCount }, () => ({
                getBoundingClientRect: () => ({
                  width: composerWidth,
                  height: composerHeight,
                }),
              }))
            },
          },
          getComputedStyle: () => ({
            visibility: composerVisible ? "visible" : "hidden",
          }),
        })(argument)
      if (sample()) return
      await new Promise((resolve) => {
        publish = () => {
          if (sample()) resolve()
        }
      })
    },
  }
  return {
    clicks,
    waits,
    page,
    publishSelection(id = intended) {
      active = id
      publish?.()
    },
    showComposer() {
      composerCount = 1
      composerWidth = 200
      composerHeight = 40
      composerVisible = true
      publish?.()
    },
  }
}

test("duplicate titles open the intended conversation in either row order", async () => {
  for (const ids of [
    [earlier, intended],
    [intended, earlier],
  ]) {
    const window = windowWith(ids)
    await openStressConversation(window.page, intended, title)
    assert.deepEqual(window.clicks, [{ id: intended, timeout: 15_000 }])
    assert.deepEqual(window.waits, [{ id: intended, timeout: 15_000 }])
  }
})

test("the previous composer's presence cannot complete a delayed selection", async () => {
  const window = windowWith([earlier, intended], { deferSelection: true })
  let ready = false
  const opening = openStressConversation(window.page, intended, title).then(() => {
    ready = true
  })
  await new Promise((resolve) => setImmediate(resolve))
  assert.deepEqual(window.waits, [{ id: earlier, timeout: 15_000 }])
  assert.equal(ready, false)
  assert.equal(window.clicks.length, 1)
  window.publishSelection()
  await opening
  assert.equal(ready, true)
  assert.equal(window.clicks.length, 1)
})

test("selection cannot be remembered across a later composer appearing for main", async () => {
  const window = windowWith([earlier, intended], {
    deferSelection: true,
    composerVisible: false,
  })
  let ready = false
  const opening = openStressConversation(window.page, intended, title).then(() => {
    ready = true
  })
  await new Promise((resolve) => setImmediate(resolve))
  window.publishSelection()
  window.publishSelection(earlier)
  window.showComposer()
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(ready, false)
  window.publishSelection()
  await opening
  assert.equal(ready, true)
  assert.equal(window.clicks.length, 1)
})

test("ambiguous composers remain a failure, including hidden duplicates", async () => {
  for (const composerVisible of [true, false]) {
    const window = windowWith([intended], { composerCount: 2, composerVisible })
    await assert.rejects(
      openStressConversation(window.page, intended, title),
      /ambiguous composers/,
    )
    assert.equal(window.clicks.length, 1)
  }
})

test("an intended selection without a visible composer is not ready", async () => {
  for (const options of [
    { composerCount: 0 },
    { composerVisible: false },
    { composerWidth: 0 },
    { composerHeight: 0 },
  ]) {
    const window = windowWith([intended], options)
    let ready = false
    const opening = openStressConversation(window.page, intended, title).then(() => {
      ready = true
    })
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(ready, false)
    window.showComposer()
    await opening
    assert.equal(ready, true)
    assert.equal(window.clicks.length, 1)
  }
})

test("an absent identity cannot open another conversation with the same title", async () => {
  const window = windowWith([earlier])
  await assert.rejects(
    openStressConversation(window.page, intended, title),
    /the requested row must be unique/,
  )
  assert.deepEqual(window.clicks, [])
  assert.deepEqual(window.waits, [])
})

/** The selected row reflects the focused pane, independent of list order/title. */
function listedWindow(rows) {
  return {
    locator(selector, options = {}) {
      return {
        async evaluateAll(read) {
          const matching = rows.filter(
            (row) =>
              (selector === css.sessionListRow ||
                (selector === css.selectedSessionListRow && row.selected)) &&
              (!options.hasText || row.title.includes(options.hasText)),
          )
          return read(
            matching.map((row) => ({
              getAttribute(name) {
                assert.equal(name, "data-session-row")
                return row.id
              },
            })),
          )
        },
      }
    },
  }
}

test("Home's focused selected identity wins over another engine's same title", async () => {
  for (const rows of [
    [
      { id: earlier, title },
      { id: intended, title, selected: true },
    ],
    [
      { id: intended, title, selected: true },
      { id: earlier, title },
    ],
  ])
    assert.equal(await selectedStressConversation(listedWindow(rows), title), intended)
})

test("delayed Home publication cannot use the previously listed same-title row", async () => {
  const rows = [{ id: earlier, title }]
  const page = listedWindow(rows)
  assert.equal(await selectedStressConversation(page, title), null)
  rows.push({ id: intended, title, selected: true })
  assert.equal(await selectedStressConversation(page, title), intended)
})

test("missing, ambiguous or differently titled Home selection supplies no identity", async () => {
  for (const rows of [
    [],
    [{ id: earlier, title }],
    [{ id: intended, title: "another conversation", selected: true }],
    [{ id: null, title, selected: true }],
    [
      { id: earlier, title, selected: true },
      { id: intended, title, selected: true },
    ],
  ])
    assert.equal(await selectedStressConversation(listedWindow(rows), title), null)
})
