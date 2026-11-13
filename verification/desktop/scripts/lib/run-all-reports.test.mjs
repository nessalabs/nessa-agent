/** Exercise the real orchestrator with disposable child scripts, without a browser/build. */
import { strict as assert } from "node:assert"
import { spawn, spawnSync } from "node:child_process"
import { once } from "node:events"
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { test } from "node:test"
import { setTimeout as delay } from "node:timers/promises"

const scripts = resolve(dirname(fileURLToPath(import.meta.url)), "..")

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "run-all-reports-test-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const lib = join(root, "lib")
  mkdirSync(lib)
  for (const file of ["cli.mjs", "server.mjs", "run-all-args.mjs", "page-lines.mjs"])
    copyFileSync(join(scripts, "lib", file), join(lib, file))
  copyFileSync(join(scripts, "run-all.mjs"), join(root, "run-all.mjs"))
  // This is the child boundary only. The orchestrator, option parser, argument
  // routing, report reads, verdict and cleanup are the actual checked-in code.
  writeFileSync(
    join(root, "perf-budget.mjs"),
    `import { writeFileSync } from 'node:fs'
const args = process.argv.slice(2)
const out = args[args.indexOf('--out') + 1]
const mode = process.env.REPORT_FIXTURE_MODE
const document = { check: 'perf-budget', results: [{ name: 'fixture', ok: mode !== 'failure', failures: mode === 'failure' ? ['fixture failed'] : [] }] }
writeFileSync(out, mode === 'hold-partial' ? '{' : JSON.stringify(document))
writeFileSync(process.env.REPORT_FIXTURE_MARKER, JSON.stringify({ out, args }))
if (mode.startsWith('hold')) setInterval(() => {}, 1000)
else process.exit(mode === 'failure' || mode === 'cleanup-failure' ? 1 : 0)
`,
  )
  const temp = join(root, "temporary")
  mkdirSync(temp)
  const aggregate = join(root, "aggregate.json")
  const parent = join(root, "retained reports")
  const marker = join(root, "child.json")
  const args = [
    join(root, "run-all.mjs"),
    "--only",
    "perf-budget",
    "--headed",
    "--layout",
    "columns,sidebar",
    "--channel",
    "bundled",
    "--runs",
    "5",
    "--out",
    aggregate,
  ]
  const env = (mode) => ({
    ...process.env,
    TMPDIR: temp,
    TMP: temp,
    TEMP: temp,
    REPORT_FIXTURE_MODE: mode,
    REPORT_FIXTURE_MARKER: marker,
  })
  const run = (mode, retained = false) =>
    spawnSync(process.execPath, [...args, ...(retained ? ["--reports", parent] : [])], {
      cwd: root,
      env: env(mode),
      encoding: "utf8",
      timeout: 10_000,
    })
  return { root, temp, aggregate, parent, marker, args, env, run }
}

for (const mode of ["success", "failure", "cleanup-failure"]) {
  test(`retention preserves the real aggregate and exit for ${mode}`, (t) => {
    const f = fixture(t)
    const ordinary = f.run(mode)
    assert.equal(ordinary.status, mode === "success" ? 0 : 1, ordinary.stderr)
    const baseline = JSON.parse(readFileSync(f.aggregate, "utf8"))
    assert.deepEqual(readdirSync(f.temp), [])
    rmSync(f.aggregate)
    const retained = f.run(mode, true)
    assert.equal(retained.status, ordinary.status, retained.stderr)
    const document = JSON.parse(readFileSync(f.aggregate, "utf8"))
    // Wall time is measured by the real orchestrator and is not a verdict.
    for (const row of baseline.summary) delete row.seconds
    for (const row of document.summary) delete row.seconds
    assert.deepEqual(document, baseline)
    assert.equal(document.ok, mode === "success")
    const child = JSON.parse(readFileSync(f.marker, "utf8"))
    assert.ok(existsSync(child.out))
    assert.equal(dirname(dirname(child.out)), f.parent)
    assert.ok(retained.stderr.includes(dirname(child.out)))
    assert.deepEqual(child.args.slice(0, 7), [
      "--layout",
      "columns,sidebar",
      "--channel",
      "bundled",
      "--headed",
      "--out",
      child.out,
    ])
    assert.ok(child.args.includes("--runs") && child.args.includes("5"))
    assert.ok(child.args.includes("--mode") && child.args.includes("prod"))
    assert.equal(child.args.includes("--reports"), false)
    if (mode === "cleanup-failure") {
      assert.equal(JSON.parse(readFileSync(child.out, "utf8")).results[0].ok, true)
      assert.equal(document.summary[0].status, "FAILED")
    }
  })
}

test("each invocation owns a fresh child and preserves parent and sibling files", (t) => {
  const f = fixture(t)
  mkdirSync(f.parent)
  const sentinel = join(f.parent, "unrelated.json")
  writeFileSync(sentinel, "parent sentinel")
  const sibling = join(f.root, "sibling")
  mkdirSync(sibling)
  writeFileSync(join(sibling, "sentinel"), "sibling sentinel")
  assert.equal(f.run("failure", true).status, 1)
  const first = JSON.parse(readFileSync(f.marker, "utf8")).out
  const firstBytes = readFileSync(first, "utf8")
  assert.equal(f.run("success", true).status, 0)
  const second = JSON.parse(readFileSync(f.marker, "utf8")).out
  assert.notEqual(dirname(first), dirname(second))
  assert.equal(readFileSync(first, "utf8"), firstBytes)
  assert.equal(readFileSync(sentinel, "utf8"), "parent sentinel")
  assert.equal(readFileSync(join(sibling, "sentinel"), "utf8"), "sibling sentinel")
  assert.equal(JSON.parse(readFileSync(second, "utf8")).results[0].ok, true)
})

test("invalid explicit report parents fail before any child, without a fallback", (t) => {
  const f = fixture(t)
  writeFileSync(f.parent, "file sentinel")
  const invalid = f.run("success", true)
  assert.notEqual(invalid.status, 0)
  assert.equal(existsSync(f.marker), false)
  assert.equal(existsSync(f.aggregate), false)
  assert.deepEqual(readdirSync(f.temp), [])
  assert.equal(readFileSync(f.parent, "utf8"), "file sentinel")
  const empty = spawnSync(process.execPath, [...f.args, "--reports", ""], {
    cwd: f.root,
    env: f.env("success"),
    encoding: "utf8",
    timeout: 10_000,
  })
  assert.equal(empty.status, 2, empty.stderr)
  assert.match(empty.stderr, /nonempty directory/)
  assert.equal(existsSync(f.marker), false)
})

for (const mode of ["hold", "hold-partial"])
  test(
    `termination keeps an already-written ${mode} report without manufacturing an aggregate`,
    {
      skip: process.platform === "win32",
      timeout: 15_000,
    },
    async (t) => {
      const f = fixture(t)
      const child = spawn(process.execPath, [...f.args, "--reports", f.parent], {
        cwd: f.root,
        env: f.env(mode),
        detached: true,
        stdio: "ignore",
      })
      const closed = once(child, "close")
      try {
        const until = Date.now() + 5_000
        while (!existsSync(f.marker) && Date.now() < until) await delay(10)
        assert.ok(existsSync(f.marker), "owned child reached its report write")
        const report = JSON.parse(readFileSync(f.marker, "utf8")).out
        process.kill(-child.pid, "SIGTERM")
        await closed
        const bytes = readFileSync(report, "utf8")
        if (mode === "hold") assert.equal(JSON.parse(bytes).results[0].ok, true)
        else assert.equal(bytes, "{")
        assert.equal(existsSync(f.aggregate), false)
      } finally {
        if (child.exitCode === null && child.signalCode === null) {
          process.kill(-child.pid, "SIGKILL")
          await closed
        }
      }
    },
  )
