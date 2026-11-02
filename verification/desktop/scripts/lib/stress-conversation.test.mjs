import assert from "node:assert/strict"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { JSDOM } from "jsdom"

import { css, selectorFor } from "./selectors.mjs"
import {
  openStressConversation,
  selectedStressConversation,
  chatIdentityTexts,
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

test("rendered UUID and named chat pill retain the same awaited identity", () => {
  const dom = new JSDOM(
    `<p>Awaited ${intended}: Echo: reply</p><p>Awaited <button data-chat-pill="${intended}"><svg><title>Claude</title></svg>Friendly chat name</button>: Echo: reply</p>`,
  )
  try {
    const lines = chatIdentityTexts([...dom.window.document.querySelectorAll("p")])
    assert.deepEqual(lines, [
      `Awaited ${intended}: Echo: reply`,
      `Awaited ${intended}: Echo: reply`,
    ])
    assert.equal(
      dom.window.document.querySelector("button").textContent,
      "ClaudeFriendly chat name",
      "the live DOM must remain untouched",
    )
  } finally {
    dom.window.close()
  }
})

test("a matching label cannot conceal the wrong target or substitute an earlier reply", () => {
  const dom = new JSDOM(
    `<p>Awaited <button data-chat-pill="${earlier}">${intended}</button>: Echo: reply</p><p>Awaited <button data-chat-pill="${intended}">Friendly chat name</button>: Echo: earlier reply</p><p>Awaited Friendly chat name: Echo: reply</p>`,
  )
  try {
    const expected = `Awaited ${intended}: Echo: reply`
    const lines = chatIdentityTexts([...dom.window.document.querySelectorAll("p")])
    assert.equal(
      lines.some((line) => line === expected),
      false,
    )
    assert.ok(lines[0].includes(earlier))
  } finally {
    dom.window.close()
  }
})
