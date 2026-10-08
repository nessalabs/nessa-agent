/**
 * The gateway stress judges a seed and a page without a gateway or a browser.
 * An incomplete list, a clipped read, a short bubble, and a scroller that
 * did not move are failures. A conversation id is the only session-row
 * selector this check builds.
 */
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { it } from "node:test"

import {
  parseScenario,
  turnFor,
} from "../../../../scripts/mcp-test-server/scripted-scenario.mjs"
import {
  gatewayStress,
  capHeld,
  listedOnPage,
  longUserText,
  scriptedReplyText,
  scrollHeld,
  seedHeld,
  sessionRowSelector,
  storedReplyText,
  transcriptHeld,
} from "./alpha-gateway.mjs"

const id = "11111111-1111-4111-8111-111111111111"
const other = "22222222-2222-4222-8222-222222222222"

function scenario() {
  return parseScenario(
    JSON.parse(
      readFileSync(
        new URL("../../fixtures/alpha-stress/reply.json", import.meta.url),
        "utf8",
      ),
    ),
  )
}

function seed(overrides = {}) {
  const longText = longUserText()
  const replyText = scriptedReplyText(scenario())
  return {
    ids: [id, other],
    longId: id,
    longText,
    turns: gatewayStress.turns,
    characters: longText.length,
    marker: gatewayStress.marker,
    listComplete: true,
    listedCount: 2,
    listedIds: [id, other],
    truncated: false,
    messageCount: gatewayStress.turns,
    viewHasLongText: true,
    replyText,
    replies: Array.from({ length: gatewayStress.turns }, () => replyText),
    ...overrides,
  }
}

it("builds an ASCII long message of the asked length", () => {
  const text = longUserText()
  assert.equal(text.length, gatewayStress.characters)
  assert.equal(text.startsWith(gatewayStress.marker), true)
  assert.equal(Buffer.byteLength(text), text.length)
  assert.equal(longUserText(), text)
})

it("builds a session-row selector only from a conversation id", () => {
  assert.equal(sessionRowSelector(id), `[data-session-row="${id}"]`)
  assert.throws(() => sessionRowSelector("load-00000"), /conversation id/)
  assert.throws(() => sessionRowSelector(`${id}"]`), /conversation id/)
  assert.throws(() => sessionRowSelector(""), /conversation id/)
})

it("accepts a complete list whose long read is the seeded text", () => {
  assert.deepEqual(seedHeld(seed()), [])
})

it("refuses an incomplete list before a page would be opened", () => {
  assert.match(seedHeld(seed({ listComplete: false }))[0], /not complete/)
})

it("refuses a list that dropped a seeded conversation", () => {
  const failures = seedHeld(seed({ listedIds: [id], listedCount: 1 }))
  assert.match(failures.join("\n"), new RegExp(other))
  assert.match(failures.join("\n"), /named 1, seeded 2/)
})

it("refuses a clipped or short long read", () => {
  assert.match(seedHeld(seed({ truncated: true }))[0], /truncated/)
  assert.match(seedHeld(seed({ messageCount: 1 }))[0], /1 messages/)
  assert.match(seedHeld(seed({ viewHasLongText: false }))[0], /does not contain/)
  assert.match(seedHeld(seed({ replies: [] }))[0], /0 agent replies/)
  assert.match(
    seedHeld(seed({ replies: Array.from({ length: gatewayStress.turns }, () => "") }))[0],
    /not the scripted text/,
  )
  assert.match(seedHeld(seed({ replyText: "" }))[0], /does not answer with text/)
})

it("holds a filled cap whose extra split arrived and added nothing", () => {
  assert.deepEqual(capHeld({ filled: 4, after: 4, cap: 4, chordArrived: true }), [])
  assert.match(
    capHeld({ filled: 3, after: 3, cap: 4, chordArrived: true })[0],
    /opened 3/,
  )
  assert.match(capHeld({ filled: 4, after: 4, cap: 4, chordArrived: false })[0], /chord/)
  assert.match(capHeld({ filled: 4, after: 5, cap: 4, chordArrived: true })[0], /left 5/)
})

it("requires every seeded conversation on the session list", () => {
  assert.deepEqual(listedOnPage([id, other], [other, id, "extra"]), [])
  assert.deepEqual(listedOnPage([id], []), [`the session list does not show ${id}`])
})

it("requires the open session's seeded bubbles, on screen, and a scroll that moved", () => {
  const text = longUserText()
  const replyText = scriptedReplyText(scenario())
  const held = {
    turns: gatewayStress.turns,
    longText: text,
    replyText,
    longId: id,
    openId: id,
    onScreen: true,
    userTexts: Array.from({ length: gatewayStress.turns }, () => text),
    agentTexts: Array.from({ length: gatewayStress.turns }, () => replyText),
    scrolled: { found: true, overflow: true, scrollTop: 400 },
  }
  assert.deepEqual(transcriptHeld(held), [])
  assert.match(transcriptHeld({ ...held, onScreen: false }).join("\n"), /not on screen/)
  assert.match(
    transcriptHeld({ ...held, openId: other }).join("\n"),
    new RegExp(`open session is ${other}`),
  )
  assert.match(
    transcriptHeld({ ...held, userTexts: [text] }).join("\n"),
    /1 user messages/,
  )
  assert.match(
    transcriptHeld({
      ...held,
      userTexts: held.userTexts.map((line) => `${line}x`),
    }).join("\n"),
    /not the seeded text/,
  )
  assert.match(transcriptHeld({ ...held, agentTexts: [] }).join("\n"), /0 agent replies/)
  assert.match(
    transcriptHeld({
      ...held,
      agentTexts: held.agentTexts.map(() => `${replyText} extra`),
    }).join("\n"),
    /not the scripted text/,
  )
  assert.match(
    transcriptHeld({
      ...held,
      scrolled: { found: true, overflow: false, scrollTop: 0 },
    }).join("\n"),
    /did not overflow/,
  )
  assert.deepEqual(scrollHeld({ found: true, overflow: true, scrollTop: 12 }), [])
  assert.deepEqual(scrollHeld(null), ["no transcript to scroll"])
})

it("the stress scenario answers a later long prompt", () => {
  const loaded = scenario()
  const prompt = longUserText()
  const first = turnFor(loaded, prompt)
  const again = turnFor(loaded, prompt)
  assert.equal(again, first)
  assert.equal(scriptedReplyText(loaded), first.steps[0].chunks.join(""))
  assert.equal(
    storedReplyText({
      parts: [
        { kind: "text", text: "Ready" },
        { kind: "thought", text: "hidden" },
        { kind: "text", text: "." },
      ],
    }),
    "Ready.",
  )
  assert.throws(
    () => scriptedReplyText({ turns: [{ steps: [{ do: "end" }] }] }),
    /does not answer/,
  )
  assert.equal(again.steps.at(-1).do, "end")
  assert.equal(turnFor(loaded, "Alpha stress pane 2").steps[0].do, "text")
})

it("run-all does not run alpha-perf", () => {
  const source = readFileSync(new URL("../run-all.mjs", import.meta.url), "utf8")
  const start = source.indexOf("const functional = [")
  const end = source.indexOf("]", start)
  assert.equal(start > 0 && end > start, true)
  assert.equal(source.slice(start, end).includes("alpha-perf"), false)
})
