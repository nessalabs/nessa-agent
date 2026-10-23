import test from "node:test"
import assert from "node:assert/strict"
import { startExperiment } from "../runtime/experiment.mjs"

test("nested uncertain browser initialization retains cleanup owner without implicit retry", async () => {
  let recovered = 0
  const primary = new Error("browser initialization failed")
  primary.cleanupError = new Error("browser close unconfirmed")
  primary.cleanupOwner = {
    close: async () => {
      recovered++
    },
  }
  let failure
  try {
    await startExperiment({
      fixture: { home: "fixture", close: async () => {} },
      openBrowser: async () => {
        throw primary
      },
      openViewer: async () => assert.fail("viewer must not start"),
    })
  } catch (error) {
    failure = error
  }
  assert.equal(recovered, 0)
  assert.equal(failure.cause, primary)
  assert.equal(failure.cleanupError, primary.cleanupError)
  await failure.cleanupOwner.close()
  assert.equal(recovered, 1)
})

test("viewer startup failure closes constructed browser before fixture", async () => {
  const closed = []
  const primary = new Error("viewer unavailable")
  await assert.rejects(
    startExperiment({
      fixture: { home: "fixture", close: async () => closed.push("fixture") },
      openBrowser: async () => ({ close: async () => closed.push("browser") }),
      openViewer: async () => {
        throw primary
      },
    }),
    (error) => error.code === "startup_failed" && error.cause === primary,
  )
  assert.deepEqual(closed, ["browser", "fixture"])
})

test("launch failure closes fixture; cleanup failure retains startup cause", async () => {
  let closed = 0
  const primary = new Error("launch unavailable")
  await assert.rejects(
    startExperiment({
      fixture: {
        home: "fixture",
        close: async () => {
          closed++
          throw new Error("fixture cleanup")
        },
      },
      openBrowser: async () => {
        throw primary
      },
      openViewer: async () => assert.fail("viewer must not start"),
    }),
    (error) =>
      error.code === "startup_failed" &&
      error.cause === primary &&
      error.cleanupError.code === "cleanup_failed",
  )
  assert.equal(closed, 1)
})

test("shutdown is idempotent and attempts remaining cleanup after failure", async () => {
  const closed = []
  const experiment = await startExperiment({
    fixture: { home: "fixture", close: async () => closed.push("fixture") },
    openBrowser: async () => ({ close: async () => closed.push("browser") }),
    openViewer: async () => ({
      close: async () => {
        closed.push("viewer")
        throw new Error("viewer cleanup")
      },
    }),
  })
  const first = experiment.close()
  assert.equal(experiment.close(), first)
  await assert.rejects(first, { code: "cleanup_failed" })
  assert.deepEqual(closed, ["viewer", "browser", "fixture"])
})
