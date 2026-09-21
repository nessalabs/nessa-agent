import assert from "node:assert/strict"
import { test } from "node:test"

import { EXPECTED, PROMPT, verdict } from "./e2e-verdict.mjs"

/** A conversation view holding exactly one turn for the prompt under test. */
function view(turn) {
  return { messages: [{ userText: PROMPT, parts: [], ...turn }], pending: [] }
}

/** A text part, which is what an answer is made of. */
const said = (text) => ({ kind: "text", text, offset: 0, toolId: "" })

test("a turn still running is waited for", () => {
  for (const status of ["queued", "running", "injected", "unresolved"])
    assert.equal(verdict(view({ status })).state, "waiting", status)
  // And a read taken before the turn is even in the view.
  assert.equal(verdict({ messages: [], pending: [] }).state, "waiting")
})

test("a run that never gets an answer is not a run that passed", () => {
  // The whole point of the check: a gateway that admits the request and then
  // produces nothing is the outage it exists to catch. Exhausting the polls
  // used to fall through to a clean exit, which made every run meaningless.
  const stuck = verdict(view({ status: "running" }))
  assert.equal(stuck.state, "waiting", "still waiting is never a pass")
  assert.notEqual(stuck.state, "answered")
})

test("a turn that ended badly is a failure, not something to keep polling", () => {
  for (const status of ["failed", "cancelled"]) {
    const answer = verdict(view({ status, error: "the provider refused" }))
    assert.equal(answer.state, "failed", status)
    assert.match(answer.detail, /the provider refused/)
  }
  // A failure with nothing said about it still reports as a failure.
  assert.equal(verdict(view({ status: "failed" })).state, "failed")
})

test("a turn that finished without saying anything has not answered", () => {
  // Completed is not answered. A turn that thought at length and said nothing,
  // and one that only ran tools, both leave the view with no reply in it.
  for (const parts of [
    [],
    [{ kind: "thought", text: "pong is what they asked for", offset: 0, toolId: "" }],
    [{ kind: "tool", text: "", offset: 0, toolId: "shell-1" }],
  ]) {
    const answer = verdict(view({ status: "completed", parts }))
    assert.equal(answer.state, "failed", JSON.stringify(parts))
    assert.match(answer.detail, /without saying/)
  }
})

test("a turn that answered something else is not the answer that was asked for", () => {
  const answer = verdict(
    view({ status: "completed", parts: [said("I cannot help with that.")] }),
  )
  assert.equal(answer.state, "failed")
  assert.match(answer.detail, /I cannot help with that/)
})

test("a completed turn carrying the word is the one result that passes", () => {
  const answer = verdict(view({ status: "completed", parts: [said("po"), said("ng")] }))
  assert.equal(answer.state, "answered")
  // Fragments are the streamed shape, so they are joined before they are read.
  assert.equal(answer.reply, EXPECTED)
  // Said among other words, and in whatever case the model chose, still counts.
  assert.equal(
    verdict(view({ status: "completed", parts: [said("Sure — Pong!")] })).state,
    "answered",
  )
})

test("only the turn this run sent is read", () => {
  // A conversation carries whatever came before. Reading the newest turn, or
  // any turn, would let an earlier answer stand in for this one.
  const earlier = {
    userText: "something else",
    status: "completed",
    parts: [said("pong")],
  }
  const mine = { userText: PROMPT, status: "running", parts: [] }
  assert.equal(verdict({ messages: [earlier, mine], pending: [] }).state, "waiting")
})
