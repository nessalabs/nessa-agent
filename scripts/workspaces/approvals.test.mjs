import { test } from "node:test"
import assert from "node:assert/strict"
import { fixtureApprovals } from "./approvals.mjs"

const review = (id, toolName = "nessa-agents/create_thread") => ({ review: id, toolName })

test("one child approval, repeated delivery and queued message preserve the four-VM fixture", async () => {
  const approvals = fixtureApprovals("delegate")
  const effects = []
  const answer = (value) =>
    approvals.answer(value, async () => effects.push(value.review))
  await answer(review("first"))
  await answer(review("first"))
  await answer(review("message", "nessa-agents/send_message"))
  await assert.rejects(answer(review("second")), /Only one delegated child/)
  assert.deepEqual(effects, ["first", "message"])
  assert.equal(approvals.size, 2)
})

test("uncertain child answer retains its reservation before another review arrives", async () => {
  const approvals = fixtureApprovals("delegate")
  let effects = 0
  let finish
  const pending = approvals.answer(review("first"), () => {
    effects += 1
    return new Promise((_, reject) => {
      finish = reject
    })
  })
  await approvals.answer(review("first"), async () => {
    effects += 1
  })
  await assert.rejects(
    approvals.answer(review("concurrent"), async () => {
      effects += 1
    }),
    /Only one delegated child/,
  )
  finish(new Error("answer outcome unknown"))
  await assert.rejects(pending, /outcome unknown/)
  await assert.rejects(
    approvals.answer(review("later"), async () => {
      effects += 1
    }),
    /Only one delegated child/,
  )
  assert.equal(effects, 1)
})

test("fixture modes retain their narrow tool approval boundaries", async () => {
  let effects = 0
  const answer = async () => {
    effects += 1
  }
  for (const [mode, name] of [
    ["none", "nessa-agents/create_thread"],
    ["log", "nessa-agents/create_thread"],
    ["delegate", "other/create_thread"],
    ["delegate", "nessa-agents/delete_thread"],
    ["delegate", "read_local_log"],
  ]) {
    await assert.rejects(
      fixtureApprovals(mode).answer(review(name, name), answer),
      /Unexpected approval/,
    )
  }
  assert.equal(effects, 0)
  await fixtureApprovals("log").answer(review("log", "fixture/read_local_log"), answer)
  assert.equal(effects, 1)
})
