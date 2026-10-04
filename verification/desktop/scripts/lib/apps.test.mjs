/**
 * `apps.mjs`'s count of one call's inline mounts: `renders` continues only on
 * exactly one, so a call drawn twice (#418) fails rather than being skipped.
 * And the window's card for an app's review of a tool: one locator for its
 * appearing and its going (W5), and `release`'s wait for a withdrawn review's card to go
 * (rows W1–W4, #436).
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  approvalCardFor,
  approvalGone,
  approvalShown,
  exactly,
  oneCard,
  oneMount,
} from "./apps.mjs"
import { css, names } from "./selectors.mjs"

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

describe("oneCard", () => {
  it("accepts exactly one step", () => {
    assert.equal(oneCard(1), null)
  })
  it("fails a call with no step", () => {
    assert.match(oneCard(0), /no step in the transcript/)
  })
  it("fails a call drawn as more than one step, naming the count", () => {
    assert.match(oneCard(3), /3 transcript steps, not one \(#418\)/)
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
 * The one locator both waits use, as `built` recorded it: cards an app asked
 * for, those whose head words are exactly the app's head for `server` and
 * `tool` and shown, the first of them. The head's `hasText` is checked by what
 * it matches, not by how it is written.
 */
function assertCardFor(built, server, tool) {
  assert.equal(built.length, 4, JSON.stringify(built))
  assert.deepEqual(built[0], ["locator", css.appApprovalCard, undefined])
  const [kind, selector, options] = built[1]
  assert.deepEqual([kind, selector], ["locator", css.approvalHeadWords])
  assert.deepEqual(Object.keys(options), ["hasText"])
  const head = names.appAsks(server, tool)
  assert.ok(options.hasText.test(head), head)
  for (const other of [
    names.appAsks(server, `${tool}s`),
    names.appAsks(server, `x${tool}`),
    names.appAsks(server, `rows ${tool}`),
    names.appAsks(server, tool.toUpperCase()),
    names.appAsks(`${server}2`, tool),
  ])
    assert.ok(!options.hasText.test(other), other)
  const [filter, { has, visible, ...rest }] = built[2]
  assert.equal(filter, "filter")
  assert.ok(has, "the card is filtered by its head")
  assert.equal(visible, true)
  assert.deepEqual(rest, {})
  assert.deepEqual(built[3], ["first"])
}

describe("exactly", () => {
  it("W5: matches the text and nothing else, case and all", () => {
    const head = names.appAsks("mcptest", "rows.delete-all")
    assert.ok(exactly(head).test(head))
    for (const other of [`${head}s`, `x${head}`, head.toUpperCase(), `${head}\n`])
      assert.ok(!exactly(head).test(other), other)
  })
  it("W5: takes the text's characters literally", () => {
    // Every character the pattern escapes, each where it would mean something.
    const literal = "^a.b*c+d?e{1}f(g)h|i[j]k$l\\m$"
    assert.ok(exactly(literal).test(literal))
    // A bare `^` past the start matches nothing, so this one is shown by a match.
    assert.ok(exactly("run ^a").test("run ^a"))
    for (const [text, unlike] of [
      ["run a.b", "run aXb"],
      ["run a*", "run "],
      ["run a+", "run aa"],
      ["run ab?", "run a"],
      ["run a{2}", "run aa"],
      ["run a|b", "run a"],
      ["run (a)", "run a"],
      ["run [ab]", "run a"],
      ["run a$", "run a"],
      ["run \\d", "run 1"],
    ])
      assert.ok(!exactly(text).test(unlike), `${text} took ${unlike}`)
  })
})

describe("approvalCardFor", () => {
  it("W5: is the first shown card an app asked for whose head names the tool", () => {
    const { built, page } = cardPage()
    approvalCardFor(page, "notes", "rows.delete-all")
    assertCardFor(built, "notes", "rows.delete-all")
  })
  it("W5: a part of a tool's name is not the tool", () => {
    // Round 7 and 8's cases: a name may hold any characters, spaces too.
    for (const [called, probe] of [
      ["rows.delete-all", "rows.delete"],
      ["rows.delete-all", "delete-all"],
      ["rows delete", "delete"],
      ["x\ny", "y"],
    ]) {
      const { built, page } = cardPage()
      approvalCardFor(page, "mcptest", probe)
      assert.ok(!built[1][2].hasText.test(names.appAsks("mcptest", called)), probe)
    }
  })
})

describe("approvalShown", () => {
  it("W5: waits for the card by the same locator", async () => {
    const { asked, built, page } = cardPage({ goesAfter: Infinity })
    assert.ok(await approvalShown(page, "mcptest", "app_delete_row", 30))
    assertCardFor(built, "mcptest", "app_delete_row")
    assert.deepEqual(asked, [{ state: "visible", timeout: 30 }])
  })
})

describe("approvalGone", () => {
  it("W1: a card already gone has gone", async () => {
    assert.equal(await approvalGone(cardPage({ goesAfter: 0 }).page, "s", "t", 50), true)
  })
  it("W2: a card still shown at the start is waited out, not sampled", async () => {
    const { asked, page } = cardPage({ goesAfter: 20 })
    assert.equal(await approvalGone(page, "s", "t", 200), true)
    assert.deepEqual(asked, [{ state: "hidden", timeout: 200 }])
  })
  it("W3: a card still shown at the bound has not gone", async () => {
    assert.equal(
      await approvalGone(cardPage({ goesAfter: Infinity }).page, "s", "t", 30),
      false,
    )
  })
  it("W4: an error other than the timeout is not the card's state", async () => {
    await assert.rejects(
      approvalGone(cardPage({ fails: true }).page, "s", "t", 30),
      /closed/,
    )
  })
  it("W5: waits on the same locator as approvalShown", async () => {
    const { built, page } = cardPage({ goesAfter: 0 })
    await approvalGone(page, "mcptest", "app_delete_row", 50)
    assertCardFor(built, "mcptest", "app_delete_row")
  })
})
