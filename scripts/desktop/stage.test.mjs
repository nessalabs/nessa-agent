import assert from "node:assert/strict"
import test from "node:test"
import { desktopStageEnvironment, resolveDesktopStage } from "./stage.mjs"

test("desktop stage defaults by command and reaches both halves", () => {
  assert.equal(resolveDesktopStage({ environment: {}, fallback: "dev" }), "dev")
  assert.equal(resolveDesktopStage({ environment: {}, fallback: "prod" }), "prod")
  assert.deepEqual(
    desktopStageEnvironment({ RETAINED: "yes", NESSA_INSTANCE: "worktree-a" }, "dev"),
    {
      RETAINED: "yes",
      NESSA_INSTANCE: "worktree-a",
      NESSA_STAGE: "dev",
      VITE_NESSA_STAGE: "dev",
    },
  )
})

test("a named stage must agree with explicit host and UI values", () => {
  assert.equal(
    resolveDesktopStage({
      environment: { NESSA_STAGE: "dev", VITE_NESSA_STAGE: "dev" },
      fallback: "prod",
      requested: "dev",
    }),
    "dev",
  )
  assert.throws(
    () =>
      resolveDesktopStage({
        environment: { NESSA_STAGE: "prod", VITE_NESSA_STAGE: "dev" },
        fallback: "prod",
      }),
    /build stage "prod".*NESSA_STAGE "prod".*VITE_NESSA_STAGE "dev"/,
  )
})

test("blank and unknown stage values are refused before Tauri starts", () => {
  for (const environment of [{ NESSA_STAGE: "" }, { VITE_NESSA_STAGE: " dev " }]) {
    assert.throws(
      () => resolveDesktopStage({ environment, fallback: "dev" }),
      /must name a stage/,
    )
  }
  assert.throws(
    () => resolveDesktopStage({ environment: {}, fallback: "dev", requested: "staging" }),
    (error) => {
      assert.match(error.message, /Unknown Nessa stage "staging"/)
      for (const stage of ["dev", "ci", "alpha", "prod"])
        assert.match(error.message, new RegExp(stage))
      return true
    },
  )
})
