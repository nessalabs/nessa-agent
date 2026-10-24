import assert from "node:assert/strict"
import { test } from "node:test"

import { css, selectorFor } from "./selectors.mjs"
import {
  openStressConversation,
  selectedStressConversation,
} from "./stress-conversation.mjs"

const title = "stress reply r1 aside s1"
const earlier = "2b3c7de2-d04a-45b6-a0bb-77cae7adb600"
const intended = "6f23287f-c023-4249-a006-b3da7824633d"

/** The page seam retains each row's identity and the composer it opens. */
function windowWith(ids) {
  const rows = ids.map((id) => ({ id, title }))
  let active
  const clicks = []
  const waits = []
  return {
    clicks,
    waits,
    page: {
      locator(selector, options) {
        if (selector === css.conversationDock)
          return {
            locator(name) {
              assert.equal(name, "textarea")
              return {
                async waitFor(options) {
                  assert.ok(active, "a row must open before its composer is awaited")
                  waits.push({ id: active, ...options })
                },
              }
            },
          }
        const matches = rows.filter(
          (row) =>
            (selector === css.sessionRow ||
              selector === selectorFor.sessionListRowFor(row.id)) &&
            (!options?.hasText || row.title.includes(options.hasText)),
        )
        const locator = (selected) => ({
          first: () => locator(selected.slice(0, 1)),
          async click(options) {
            assert.equal(selected.length, 1, "the requested row must be unique")
            active = selected[0].id
            clicks.push({ id: active, ...options })
          },
        })
        return locator(matches)
      },
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

function modelPicker(groups, selected = 0) {
  let opened = false,
    chosen
  const visited = []
  const page = {
    locator(selector) {
      if (selector === css.modelProviderTab)
        return {
          count: async () => groups.length,
          nth: (i) => ({
            click: async () => {
              selected = i
              visited.push(i)
            },
          }),
        }
      assert.equal(selector, css.modelOption)
      return {
        first: () => ({ waitFor: async () => assert.equal(opened, true) }),
        filter: ({ hasText }) => ({
          count: async () =>
            groups[selected].filter((name) => name.includes(hasText)).length,
          click: async () => {
            chosen = groups[selected].find((name) => name.includes(hasText))
          },
        }),
      }
    },
  }
  return {
    page,
    trigger: {
      click: async () => {
        opened = true
      },
    },
    visited,
    chosen: () => chosen,
  }
}
test("Home selects the exact fixture across provider tabs, never the initial provider default", async () => {
  const { selectStressModel } = await import("./stress-conversation.mjs")
  const picker = modelPicker([["Claude Opus 5"], ["GPT-6 Astra", "GPT-5.6 Terra"]])
  await selectStressModel(picker.page, picker.trigger, "GPT-5.6 Terra")
  assert.equal(picker.chosen(), "GPT-5.6 Terra")
  assert.deepEqual(picker.visited, [0, 1])
})
test("Home missing or ambiguous fixture models cannot fall back to another selection", async () => {
  const { selectStressModel } = await import("./stress-conversation.mjs")
  for (const groups of [
    [["Claude Opus 5"], ["GPT-6 Astra"]],
    [["GPT-5.6 Terra", "GPT-5.6 Terra duplicate"]],
  ]) {
    const picker = modelPicker(groups)
    await assert.rejects(
      selectStressModel(picker.page, picker.trigger, "GPT-5.6 Terra"),
      /not uniquely offered/,
    )
    assert.equal(picker.chosen(), undefined)
  }
})
