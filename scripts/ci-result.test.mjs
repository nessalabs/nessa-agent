import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import test from "node:test"

import { ALWAYS_RUN, evaluateChecks } from "./ci-result.mjs"

function needs(code, jobs) {
  return {
    changes: { result: "success", outputs: { code } },
    ...Object.fromEntries(
      Object.entries(jobs).map(([job, result]) => [job, { result, outputs: {} }]),
    ),
  }
}

const compileJobs = {
  workflows: "success",
  "gateway-contract": "success",
  "desktop-release-profile": "success",
  frontend: "success",
  "local-auth": "success",
  "sdk-domain-coverage": "success",
}

test("a code change passes only when every owned job ran and passed", () => {
  assert.equal(evaluateChecks(needs("true", compileJobs)).ok, true)
})

test("a skipped compile job fails a code change", () => {
  const result = evaluateChecks(
    needs("true", { ...compileJobs, "sdk-domain-coverage": "skipped" }),
  )
  assert.equal(result.ok, false)
  assert.deepEqual(result.failures, [
    {
      job: "sdk-domain-coverage",
      result: "skipped",
      reason: "a code change requires this job to run and pass",
    },
  ])
})

test("a documentation-only change may skip compile jobs and nothing else", () => {
  const skipped = Object.fromEntries(
    Object.keys(compileJobs).map((job) => [
      job,
      job === "workflows" ? "success" : "skipped",
    ]),
  )
  assert.equal(evaluateChecks(needs("false", skipped)).ok, true)

  const workflowsSkipped = evaluateChecks(
    needs("false", { ...skipped, workflows: "skipped" }),
  )
  assert.equal(workflowsSkipped.ok, false)
  assert.equal(workflowsSkipped.failures[0].job, "workflows")

  const compileFailed = evaluateChecks(
    needs("false", { ...skipped, "local-auth": "failure" }),
  )
  assert.equal(compileFailed.ok, false)
  assert.equal(compileFailed.failures[0].job, "local-auth")
})

test("a missing or failed classification fails closed", () => {
  assert.equal(evaluateChecks({ workflows: { result: "success" } }).ok, false)
  assert.equal(
    evaluateChecks({
      changes: { result: "failure", outputs: {} },
      workflows: { result: "success" },
    }).ok,
    false,
  )
  assert.equal(evaluateChecks(needs(undefined, compileJobs)).ok, false)
  assert.equal(evaluateChecks(needs(true, compileJobs)).ok, false)
  for (const value of [null, [], "needs"]) assert.equal(evaluateChecks(value).ok, false)
})

test("the process prints the result on stdout and exits non-zero on failure", () => {
  const passed = spawnSync(process.execPath, ["scripts/ci-result.mjs"], {
    cwd: new URL("..", import.meta.url),
    env: { ...process.env, NEEDS: JSON.stringify(needs("true", compileJobs)) },
    encoding: "utf8",
  })
  assert.equal(passed.status, 0)
  assert.equal(JSON.parse(passed.stdout).ok, true)
  assert.equal(passed.stderr, "")

  const failed = spawnSync(process.execPath, ["scripts/ci-result.mjs"], {
    cwd: new URL("..", import.meta.url),
    env: {
      ...process.env,
      NEEDS: JSON.stringify(needs("true", { ...compileJobs, "local-auth": "failure" })),
    },
    encoding: "utf8",
  })
  assert.equal(failed.status, 1)
  assert.equal(JSON.parse(failed.stdout).ok, false)
  assert.match(failed.stderr, /^local-auth: failure: /)

  const unreadable = spawnSync(process.execPath, ["scripts/ci-result.mjs"], {
    cwd: new URL("..", import.meta.url),
    env: { ...process.env, NEEDS: "{" },
    encoding: "utf8",
  })
  assert.equal(unreadable.status, 1)
  assert.equal(JSON.parse(unreadable.stdout).ok, false)
})

function jobBlocks(workflow) {
  const body = workflow.slice(workflow.search(/^jobs:\r?\n/m))
  const ids = [...body.matchAll(/^  ([a-z][a-z0-9-]*):\r?$/gm)]
  return ids.map((match, index) => ({
    id: match[1],
    text: body.slice(
      match.index,
      index + 1 < ids.length ? ids[index + 1].index : body.length,
    ),
  }))
}

test("the aggregate job names every other job, and only the unguarded jobs always run", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  const blocks = jobBlocks(workflow)
  const aggregate = blocks.find((block) => block.id === "required-checks")
  assert.ok(aggregate, "required-checks job is missing")
  const needs = [...aggregate.text.matchAll(/^\s+- ([a-z0-9-]+)\r?$/gm)].map(
    (match) => match[1],
  )
  assert.deepEqual(
    needs.toSorted(),
    blocks
      .map((block) => block.id)
      .filter((id) => id !== "required-checks")
      .toSorted(),
  )
  assert.match(aggregate.text, /always\(\) && !cancelled\(\)/)
  assert.match(aggregate.text, /node scripts\/ci-result\.mjs/)

  const unguarded = blocks
    .filter((block) => block.id !== "required-checks")
    .filter((block) => !block.text.includes("needs.changes.outputs.code"))
    .map((block) => block.id)
  assert.deepEqual(unguarded.toSorted(), [...ALWAYS_RUN].toSorted())
})
