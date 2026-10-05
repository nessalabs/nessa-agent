/**
 * The scripted run's verdict and summary, with no gateway and no browser:
 * one line on stdout's shape, a skip that is not "could not run", and page
 * lines taken from the arrays the checks already record.
 */
import { strict as assert } from "node:assert"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

import {
  enginesFrom,
  exitOf,
  overallVerdict,
  prSummary,
  relevantLines,
  scriptedCheckArgs,
  verdictLine,
  writeView,
} from "./scripted-evidence.mjs"

test("the verdict line is one JSON object naming the evidence and the summary", () => {
  const line = verdictLine({
    verdict: "pass",
    evidence: "/tmp/evidence",
    summary: "/tmp/evidence/pr-summary.md",
  })
  assert.equal(line.endsWith("\n"), true)
  assert.equal(line.trim().split("\n").length, 1)
  assert.deepEqual(JSON.parse(line), {
    verdict: "pass",
    evidence: "/tmp/evidence",
    summary: "/tmp/evidence/pr-summary.md",
  })
  assert.equal(exitOf("pass"), 0)
  assert.equal(exitOf("fail"), 1)
  assert.equal(exitOf("could-not-run"), 2)
})

test("a failed check wins, and a skipped dev-only check is not could-not-run", () => {
  const skipped = { name: "mcp-apps-gateway", skipped: "not run: dev server only" }
  assert.equal(
    overallVerdict([
      skipped,
      { name: "gateway-window", status: 0 },
      { name: "scripted-scenarios", status: 0 },
    ]),
    "pass",
  )
  assert.equal(
    overallVerdict([
      skipped,
      { name: "gateway-window", status: 1 },
      { name: "scripted-scenarios", status: 2 },
    ]),
    "fail",
  )
  assert.equal(
    overallVerdict([
      skipped,
      { name: "gateway-window", status: 0 },
      { name: "scripted-scenarios", status: 2 },
    ]),
    "could-not-run",
  )
  assert.equal(overallVerdict([skipped]), "could-not-run")
  assert.equal(
    overallVerdict([{ name: "gateway-window", status: null }]),
    "could-not-run",
  )
})

test("engines and page lines come from the check's own results", () => {
  const document = {
    results: [
      { name: "setup", ok: true, failures: [] },
      {
        name: "allow",
        engine: "chromium",
        ok: false,
        failures: ["the card was missing", "console.error: boom"],
        harmless: [
          "requestfailed: http://127.0.0.1/x net::ERR_ABORTED (aborted after a 204 response, #485)",
        ],
      },
      { name: "allow", engine: "webkit", ok: true, cannotRun: false, failures: [] },
      {
        name: "launch",
        engine: "webkit",
        ok: false,
        cannotRun: true,
        error: "could not run: no webkit",
      },
    ],
  }
  assert.deepEqual(enginesFrom(document), { chromium: "fail", webkit: "could-not-run" })
  const lines = relevantLines({
    results: document.results,
    verdict: "fail",
    gatewayLog: "a\nb\n",
  })
  assert.equal(lines.includes("allow chromium: the card was missing"), true)
  assert.equal(lines.includes("allow chromium: console.error: boom"), true)
  assert.equal(
    lines.some((line) => line.includes("requestfailed:") && line.endsWith("(harmless)")),
    true,
  )
  assert.equal(lines.at(-1), "b")
  const quiet = relevantLines({
    results: [
      {
        name: "allow",
        engine: "chromium",
        failures: [],
        harmless: ["requestfailed: http://x net::ERR"],
      },
    ],
    verdict: "pass",
    gatewayLog: "secret gateway line",
  })
  assert.deepEqual(quiet, ["allow chromium: requestfailed: http://x net::ERR (harmless)"])
})

test("the summary names the verdict, each engine, and the lines", () => {
  const text = prSummary({
    verdict: "pass",
    checks: [
      {
        name: "mcp-apps-gateway",
        skipped: "not run: dev server only",
      },
      { name: "gateway-window", engines: { chromium: "pass", webkit: "pass" } },
    ],
    lines: ["allow chromium: console.error: boom"],
  })
  assert.match(text, /^Verdict: pass/)
  assert.match(
    text,
    /\| mcp-apps-gateway \| not run: dev server only \| not run: dev server only \|/,
  )
  assert.match(text, /\| gateway-window \| pass \| pass \|/)
  assert.match(text, /- allow chromium: console\.error: boom/)
})

test("writeView stores the view as JSON under the evidence directory", () => {
  const directory = mkdtempSync(join(tmpdir(), "scripted-evidence-"))
  const path = writeView(directory, "views/permission.json", { status: "running" })
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), { status: "running" })
})

test("a bundled Chromium channel is forwarded on the child check's argv", () => {
  const args = scriptedCheckArgs({
    script: "gateway-window.mjs",
    agent: "claude",
    mode: "dev",
    channel: "bundled",
    evidence: "/tmp/evidence",
    shots: "/tmp/evidence/shots",
    out: "/tmp/evidence/result.json",
    extra: ["--scripted"],
  })
  assert.equal(args[args.indexOf("--channel") + 1], "bundled")
  assert.equal(args.at(-1), "--scripted")
})

test("the scripted command accepts --channel bundled", () => {
  const script = join(dirname(fileURLToPath(import.meta.url)), "../scripted-e2e.mjs")
  const unknown = spawnSync(
    process.execPath,
    [script, "--channel", "bundled", "--nope"],
    { encoding: "utf8" },
  )
  assert.equal(unknown.status, 2)
  assert.match(unknown.stderr, /Unknown option '--nope'/)

  const accepted = spawnSync(
    process.execPath,
    [script, "--channel", "bundled", "--mode", "nope"],
    { encoding: "utf8" },
  )
  assert.equal(accepted.status, 2)
  assert.doesNotMatch(accepted.stderr, /unknown option/)
  assert.match(accepted.stderr, /--channel is chrome or bundled/)
})
