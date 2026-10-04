/**
 * `apps.mjs`'s count of one call's inline mounts: `renders` continues only on
 * exactly one, so a call drawn twice (#418) fails rather than being skipped.
 * And the window's approval card naming a tool: one locator for its appearing
 * and its going (W5), and `release`'s wait for a withdrawn review's card to go
 * (rows W1–W4, #436).
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  approvalCardFor,
  approvalGone,
  approvalShown,
  oneMount,
  toolAtEnd,
} from "./apps.mjs"
import { css } from "./selectors.mjs"

describe("oneMount", () => {
  it("accepts exactly one inline frame", () => {
    assert.equal(oneMount(1), null)
  })
  it("fails a call with no frame", () => {
    assert.match(oneMount(0), /no inline app frame/)
  })
  it("fails a call drawn more than once, naming the count", () => {
    assert.match(oneMount(2), /2 inline app frames, not one \(#418\)/)
    assert.match(oneMount(4), /4 inline app frames/)
  })
})

/**
 * A page whose one approval card goes `goesAfter` ms from the wait's start
 * (`0`: already gone, `Infinity`: never), the way Playwright's
 * `waitFor({ state: "hidden" })` settles: when it goes, or a `TimeoutError`
 * at the bound. `fails` makes the wait throw another error. `built` records
 * how the locator was made: the selector and options, each filter, `.first()`.
 */
function cardPage({ goesAfter, fails } = {}) {
  const asked = []
  const built = []
  const locator = {
    filter: (options) => {
      built.push(["filter", options])
      return locator
    },
    first: () => {
      built.push(["first"])
      return locator
    },
    // Shown at the start unless already gone: what a sample would see.
    isVisible: () => Promise.resolve(goesAfter > 0),
    waitFor: ({ state, timeout }) => {
      asked.push({ state, timeout })
      if (fails) return Promise.reject(new Error("Target page has been closed"))
      if (state !== "hidden") return Promise.resolve()
      if (goesAfter > timeout) {
        const error = new Error(`Timeout ${timeout}ms exceeded`)
        error.name = "TimeoutError"
        return new Promise((_, reject) => setTimeout(() => reject(error), timeout))
      }
      return new Promise((resolve) => setTimeout(resolve, goesAfter))
    },
  }
  const page = {
    locator: (selector, options) => {
      built.push(["locator", selector, options])
      return locator
    },
  }
  return { asked, built, page }
}

/**
 * The one locator both waits use, as `built` recorded it: approval cards,
 * those with head words naming `tool` and shown, the first of them. The head's
 * `hasText` is checked by what it matches, not by how it is written.
 */
function assertCardFor(built, tool) {
  assert.equal(built.length, 4, JSON.stringify(built))
  assert.deepEqual(built[0], ["locator", css.approvalCard, undefined])
  const [kind, selector, options] = built[1]
  assert.deepEqual([kind, selector], ["locator", css.approvalHeadWords])
  assert.deepEqual(Object.keys(options), ["hasText"])
  assert.ok(options.hasText.test(`The mcptest app wants to run ${tool}`))
  assert.ok(!options.hasText.test(`The mcptest app wants to run ${tool}s`))
  assert.ok(!options.hasText.test(`The mcptest app wants to run ${tool.toUpperCase()}`))
  const [filter, { has, visible, ...rest }] = built[2]
  assert.equal(filter, "filter")
  assert.ok(has, "the card is filtered by its head")
  assert.equal(visible, true)
  assert.deepEqual(rest, {})
  assert.deepEqual(built[3], ["first"])
}

describe("toolAtEnd", () => {
  it("W5: names the tool as the head's last whole name, case and all", () => {
    const names = toolAtEnd("app_delete_row")
    assert.ok(names.test("The mcptest app wants to run app_delete_row"))
    assert.ok(names.test("app_delete_row"))
    for (const other of [
      "The mcptest app wants to run app_delete_rows",
      "The mcptest app wants to run APP_DELETE_ROW",
      "The mcptest app wants to run xapp_delete_row",
      "The mcptest app wants to run app_delete_row now",
    ])
      assert.ok(!names.test(other), other)
  })
  it("W5: bounds a name by the head, whatever characters it holds", () => {
    const head = "The mcptest app wants to run rows.delete-all"
    assert.ok(toolAtEnd("rows.delete-all").test(head))
    for (const part of ["rows.delete", "rows", "delete-all", "all"])
      assert.ok(!toolAtEnd(part).test(head), part)
  })
  it("W5: takes a tool's characters literally", () => {
    assert.ok(toolAtEnd("a.b(c)").test("run a.b(c)"))
    assert.ok(!toolAtEnd("a.b").test("run aXb"))
  })
})

describe("approvalCardFor", () => {
  it("W5: is the first shown approval card whose head names the tool", () => {
    const { built, page } = cardPage()
    approvalCardFor(page, "app_delete_row")
    assertCardFor(built, "app_delete_row")
  })
})

describe("approvalShown", () => {
  it("W5: waits for the card by the same locator", async () => {
    const { asked, built, page } = cardPage({ goesAfter: Infinity })
    assert.ok(await approvalShown(page, "app_delete_row", 30))
    assertCardFor(built, "app_delete_row")
    assert.deepEqual(asked, [{ state: "visible", timeout: 30 }])
  })
})

describe("approvalGone", () => {
  it("W1: a card already gone has gone", async () => {
    assert.equal(await approvalGone(cardPage({ goesAfter: 0 }).page, "t", 50), true)
  })
  it("W2: a card still shown at the start is waited out, not sampled", async () => {
    const { asked, page } = cardPage({ goesAfter: 20 })
    assert.equal(await approvalGone(page, "t", 200), true)
    assert.deepEqual(asked, [{ state: "hidden", timeout: 200 }])
  })
  it("W3: a card still shown at the bound has not gone", async () => {
    assert.equal(
      await approvalGone(cardPage({ goesAfter: Infinity }).page, "t", 30),
      false,
    )
  })
  it("W4: an error other than the timeout is not the card's state", async () => {
    await assert.rejects(approvalGone(cardPage({ fails: true }).page, "t", 30), /closed/)
  })
  it("W5: waits on the same locator as approvalShown", async () => {
    const { built, page } = cardPage({ goesAfter: 0 })
    await approvalGone(page, "app_delete_row", 50)
    assertCardFor(built, "app_delete_row")
  })
})
