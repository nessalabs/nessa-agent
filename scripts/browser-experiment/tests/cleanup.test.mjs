import test from "node:test"
import assert from "node:assert/strict"
import { mkdtemp, writeFile, readFile, rm, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { initializeBrowser } from "../runtime/browser.mjs"
import { finishVerification } from "./finish-verification.mjs"
import { ExperimentFailure } from "../runtime/session.mjs"

test("browser initialization retains primary failure, cleanup error and recovery owner", async () => {
  const primary = new Error("navigation failed")
  let closes = 0
  const page = {
    on() {},
    goto: async () => {
      throw primary
    },
  }
  const context = {
    pages: () => [page],
    on() {},
    close: async () => {
      closes++
      if (closes === 1) throw new Error("close failed")
    },
  }
  let failure
  try {
    await initializeBrowser(context, { downloads: "unused", home: "fixture" })
  } catch (error) {
    failure = error
  }
  assert.equal(failure.code, "browser_initialization_failed")
  assert.equal(failure.cause.code, "browser_operation_failed")
  assert.equal(failure.cause.cause, primary)
  assert.equal(failure.cleanupError.code, "cleanup_failed")
  await failure.cleanupOwner.close()
  assert.equal(closes, 2)
})

test("unconfirmed browser cleanup preserves test tree and writes primary plus cleanup failures", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-1089-cleanup-"))
  const marker = join(root, "test-profile")
  const artifact = join(root, "report.json")
  await writeFile(marker, "test-only")
  const report = { result: "failed", error: { message: "primary test failure" } }
  try {
    await finishVerification({
      report,
      root,
      close: async () => {
        throw new Error("browser close unconfirmed")
      },
      removeProfile: async () => rm(marker),
      writeReport: async (value) => writeFile(artifact, JSON.stringify(value)),
    })
    assert.ok(await stat(marker))
    const saved = JSON.parse(await readFile(artifact, "utf8"))
    assert.equal(saved.error.message, "primary test failure")
    assert.equal(saved.cleanupError.message, "browser close unconfirmed")
    assert.equal(saved.cleanedTestProfiles, false)
    assert.equal(saved.retainedTestRoot, root)
    await finishVerification({
      report,
      root,
      close: async () => {
        throw new ExperimentFailure(
          "cleanup_failed",
          new AggregateError([new Error("context termination unconfirmed")]),
        )
      },
      removeProfile: async () => assert.fail("must retain profile"),
      writeReport: async (value) =>
        assert.equal(
          value.cleanupError.cause.errors[0].message,
          "context termination unconfirmed",
        ),
    })
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("confirmed close removes profile before writing result; removal failure is reported", async () => {
  const steps = []
  const report = { result: "blocked" }
  await finishVerification({
    report,
    root: "test-root",
    close: async () => steps.push("close"),
    removeProfile: async () => steps.push("remove"),
    writeReport: async () => steps.push("report"),
  })
  assert.deepEqual(steps, ["close", "remove", "report"])
  assert.equal(report.cleanedTestProfiles, true)
  await finishVerification({
    report: { result: "passed" },
    root: "test-root",
    close: async () => {},
    removeProfile: async () => {
      throw new Error("remove failed")
    },
    writeReport: async (value) => {
      assert.equal(value.result, "failed")
      assert.equal(value.cleanedTestProfiles, false)
      assert.equal(value.cleanupError.message, "remove failed")
    },
  })
})

test("unconfirmed secondary context prevents deletion after ordinary resources close", async () => {
  const report = {
    result: "failed",
    error: { message: "secondary context close failed" },
  }
  let removed = false
  let written = false
  await finishVerification({
    report,
    root: "shared-test-profile",
    close: async () => {},
    hasUnconfirmedResources: () => true,
    removeProfile: async () => {
      removed = true
    },
    writeReport: async (value) => {
      written = true
      assert.equal(value.error.message, "secondary context close failed")
      assert.equal(value.cleanupError.code, "browser_cleanup_unconfirmed")
    },
  })
  assert.equal(removed, false)
  assert.equal(written, true)
  assert.equal(report.cleanedTestProfiles, false)
})
