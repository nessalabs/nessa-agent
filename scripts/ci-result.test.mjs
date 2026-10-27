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
  "windows-server-tests": "success",
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

test("the Windows split preserves full build flags, platform coverage, guards, caches and permissions", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  for (const checkout of [workflow, workflow.replace(/\r?\n/g, "\r\n")]) {
    const blocks = jobBlocks(checkout)
    const local = blocks.find((block) => block.id === "local-auth").text
    const server = blocks.find((block) => block.id === "windows-server-tests").text
    const command =
      /run: node scripts\/cargo-test-parallel\.mjs --concurrency (\d+) --partition (.+?) -- (.+)\r?$/m
    const localCommand = local.match(command)
    const serverCommand = server.match(command)
    assert.ok(localCommand)
    assert.ok(serverCommand)
    assert.equal(localCommand[1], "2")
    assert.equal(serverCommand[1], "2")
    assert.equal(localCommand[2], "${{ matrix.partition }}")
    assert.equal(serverCommand[2], "server-lib")
    assert.equal(localCommand[3], serverCommand[3])
    assert.equal(
      localCommand[3],
      "-p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-device-client -p nessa-sdk",
    )
    assert.match(local, /fail-fast: false/)
    assert.match(local, /os: \[windows-latest, ubuntu-24\.04, macos-latest\]/)
    for (const [os, partition] of [
      ["windows-latest", "remaining"],
      ["ubuntu-24\\.04", "all"],
      ["macos-latest", "all"],
    ])
      assert.match(local, new RegExp(`- os: ${os}\\s+partition: ${partition}`))
    assert.match(server, /runs-on: windows-latest/)
    for (const block of [local, server]) {
      assert.match(block, /needs: changes/)
      assert.match(
        block,
        /if: \$\{\{ !cancelled\(\) && needs\.changes\.outputs\.code != 'false' \}\}/,
      )
      assert.match(
        block,
        /github\.event_name != 'workflow_dispatch' \|\| github\.event\.inputs\.rust-cache != 'cold'/,
      )
      assert.match(block, /components: clippy,rustfmt/)
      assert.match(block, /node-version: 24/)
      assert.doesNotMatch(block, /permissions:|continue-on-error:|--test-threads|--skip/)
    }
    assert.match(local, /save-if: \$\{\{ github\.ref == 'refs\/heads\/main' \}\}/)
    assert.match(server, /shared-key: local-auth\r?\n\s+save-if: false/)
    assert.match(checkout, /permissions:\r?\n  contents: read\r?\n/)
    assert.match(
      checkout,
      /cancel-in-progress: \$\{\{ github\.event_name == 'pull_request' \}\}/,
    )
    assert.match(
      blocks.find((block) => block.id === "gateway-contract").text,
      /node --test scripts\/cargo-test-parallel\/e2e\.test\.mjs scripts\/cargo-test-parallel\/diagnostics\.test\.mjs/,
    )
    assert.doesNotMatch(
      server,
      /--doc|clippy -p|check-desktop|smoke-auth|check-windows-task-scheduler/,
    )
    for (const check of [
      "cargo clippy -p nessa-local-storage",
      "node scripts/check-desktop.mjs",
      "check-windows-task-scheduler.ps1",
      "scripts/smoke-auth.mjs",
      "scripts/smoke-windows-private-files.ts",
    ])
      assert.ok(local.includes(check), check)
  }
})

test("either Windows partition failure or unexpected skip fails the aggregate", () => {
  for (const job of ["windows-server-tests", "local-auth"]) {
    for (const result of ["failure", "cancelled", "skipped"]) {
      const evaluated = evaluateChecks(needs("true", { ...compileJobs, [job]: result }))
      assert.equal(evaluated.ok, false)
      assert.equal(evaluated.failures[0].job, job)
    }
  }
})
