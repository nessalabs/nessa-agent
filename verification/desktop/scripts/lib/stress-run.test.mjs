import assert from "node:assert/strict"
import { test } from "node:test"
import { BackgroundDrainTimeout, Run } from "./stress-run.mjs"

const runWith = (wait) => new Run({}, { page: {} }, {}, () => {}, wait)

test("a confirmed drain runs once with the original deadline and blocks background entry", async () => {
  let run,
    resumed = false,
    entering,
    called = 0
  run = runWith(async (ready, deadline) => {
    assert.equal(deadline, 120_000)
    assert.equal(ready(), false)
    entering = run.enter().then(() => {
      resumed = true
    })
    await Promise.resolve()
    assert.equal(resumed, false)
    run.leave()
    assert.equal(ready(), true)
    return true
  })
  run.busy = 1
  const result = await run.exclusive(async () => {
    called += 1
    assert.equal(run.busy, 0)
    assert.equal(resumed, false)
    return "done"
  })
  await entering
  assert.equal(result, "done")
  assert.equal(called, 1)
  assert.equal(resumed, true)
  assert.equal(run.busy, 1)
  assert.equal(run.paused, null)
})

test("an expired drain never starts the fixture and releases waiting background work", async () => {
  let called = 0,
    entering
  const run = runWith(async (ready, deadline) => {
    assert.equal(deadline, 120_000)
    assert.equal(ready(), false)
    entering = run.enter()
    return false
  })
  run.busy = 1
  await assert.rejects(
    run.exclusive(() => {
      called += 1
    }),
    BackgroundDrainTimeout,
  )
  await entering
  assert.equal(called, 0)
  assert.equal(run.paused, null)
  assert.equal(run.busy, 2)
})

test("a failed drain preserves its error, skips the fixture, and releases the pause", async () => {
  const original = new Error("drain failed")
  let called = 0
  const run = runWith(async () => {
    throw original
  })
  await assert.rejects(
    run.exclusive(() => {
      called += 1
    }),
    (error) => error === original,
  )
  assert.equal(called, 0)
  assert.equal(run.paused, null)
  await run.enter()
  assert.equal(run.busy, 1)
})

test("a failed fixture preserves its error and releases the pause", async () => {
  const original = new Error("fixture failed")
  const run = runWith(async () => true)
  await assert.rejects(
    run.exclusive(async () => {
      throw original
    }),
    (error) => error === original,
  )
  assert.equal(run.paused, null)
  await run.enter()
  assert.equal(run.busy, 1)
})
