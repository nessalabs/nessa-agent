#!/usr/bin/env node
/**
 * Runs every desktop check and summarises them. The functional checks share
 * one page (the dev server by default); perf-budget and message-sync each
 * build and preview production themselves, as their budgets require.
 * Takes minutes — run it before
 * handing off UI work, not on every change; while iterating, run the check
 * that covers the change with `--quick` (one engine, layout and size).
 */
import { spawn } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import {
  checksUnder,
  chosen,
  cli,
  DEV_SERVER_ONLY,
  devServerOnlyChecks,
  log,
  overallStatus,
  table,
  UsageError,
  verbose,
  verdictOf,
} from "./lib/cli.mjs"
import { functionalPreviewEnv, target } from "./lib/server.mjs"

const here = dirname(fileURLToPath(import.meta.url))
// workspace-load.mjs stays off this list. CHECKLIST.md › Seeded workspace load.
const functional = [
  "smoke",
  "icon-buttons",
  "shared-controls",
  "focus",
  "drag",
  "responsive",
  "safe-area",
  "columns",
  "committed-transcript",
  "lease-details",
  "load-fallback",
  "gateway-states",
  "conversation-unread",
  "mode-publication",
  "panel-list-follow",
  "linked-devices",
  "settings-page",
  "widgets",
  "subagents",
  "mcp-apps",
  "app-review",
  "message-sync",
  "command-order",
]

const options = cli({
  name: "run-all",
  summary: "every desktop verification check, summarised",
  options: {
    only: { type: "string" },
    "skip-perf": { type: "boolean", default: false },
    runs: { type: "string" },
  },
  help: `
Usage: node verification/desktop/scripts/run-all.mjs [options]

  --only <list>    Checks to run (default: ${[...functional, "perf-budget"].join(", ")});
                   a name not among them is refused (exit 2).
  --skip-perf      Leave out perf-budget (the production build and its runs).
  --runs <n>       Passed to perf-budget.

message-sync builds its own production fixture preview in either mode; its
retained-app functional check is run separately in dev with the sandbox listener.
--url / --mode apply to the functional checks; perf-budget always measures a
production build unless --url is given. A production functional preview is
still minified, and its inlined application stage is ci
(functionalPreviewEnv) so a scripted loopback browser socket is admitted.
perf-budget keeps the prod stage. --mode prod leaves out the checks
that need the dev server (${devServerOnlyChecks.join(", ")}) and names them
"${DEV_SERVER_ONLY}". That is not "could not run". Steps inside drag and
widgets that read the dev server's modules are left out the
same way. --engine and --layout, when given, are passed to every check;
otherwise each uses its own default. Each check's JSON is collected into
one document on stdout (or --out).

Exit: 0 every check that ran held; 1 any check failed (whatever else could
not run or was left out); 2 nothing failed but a check that ran could not
run, or nothing ran.`,
})

const passThrough = () => {
  const args = []
  const given = new Set(
    process.argv.slice(2).map((a) => a.replace(/^--(no-)?/, "").split("=")[0]),
  )
  for (const key of ["engine", "layout", "channel", "shots"])
    if (given.has(key) && options[key]) args.push(`--${key}`, options[key])
  if (options.headed) args.push("--headed")
  if (options.quick) args.push("--quick")
  if (options.verbose) args.push("--verbose")
  return args
}

/** Runs one check; resolves with its exit code (null for a signal) and time, never rejects. */
function run(check, args) {
  return new Promise((ok) => {
    const started = Date.now()
    const seconds = () => Math.round((Date.now() - started) / 1000)
    let proc
    try {
      proc = spawn(process.execPath, [join(here, `${check}.mjs`), ...args], {
        stdio: ["ignore", "ignore", "pipe"],
      })
    } catch (error) {
      log(`[${check}] did not start: ${error.message}`)
      return ok({ code: null, seconds: seconds() })
    }
    let partial = ""
    proc.stderr.on("data", (chunk) => {
      const lines = (partial + chunk.toString()).split("\n")
      partial = lines.pop()
      for (const line of lines) process.stderr.write(`[${check}] ${line}\n`)
    })
    proc.stderr.on(
      "end",
      () => partial && process.stderr.write(`[${check}] ${partial}\n`),
    )
    proc.on("error", (error) => {
      log(`[${check}] did not start: ${error.message}`)
      ok({ code: null, seconds: seconds() })
    })
    proc.on("exit", (code) => ok({ code, seconds: seconds() }))
  })
}

const all = [...functional, "perf-budget"]
let checks
try {
  checks = options.only
    ? chosen(options.only, all, options.list)
    : [...functional, ...(options["skip-perf"] ? [] : ["perf-budget"])]
} catch (error) {
  if (!(error instanceof UsageError)) throw error
  log(`run-all: ${error.message}`)
  process.exit(2)
}
const scheduled = checksUnder(options.mode, checks)
const leftOutSet = new Set(scheduled.leftOut)
const sharesPage = (check) => functional.includes(check) && check !== "message-sync"
const dir = mkdtempSync(join(tmpdir(), "nessa-desktop-verify-all-"))
const summary = []
const documents = {}
let page
try {
  if (scheduled.run.some(sharesPage)) {
    try {
      page = await target(options, options.mode === "prod" ? functionalPreviewEnv() : {})
    } catch (error) {
      // No page to test. Each functional check is recorded in the loop below,
      // in check order, so a left-out row stays where that check sits.
      // perf-budget builds its own page.
      log(`run-all: could not start the page: ${error.message}`)
    }
  }
  for (const check of checks) {
    if (leftOutSet.has(check)) {
      log(`\n=== ${check}`)
      log(`${check}: ${DEV_SERVER_ONLY}`)
      summary.push({ check, status: DEV_SERVER_ONLY, held: "—", seconds: 0 })
      continue
    }
    if (sharesPage(check) && !page) {
      summary.push({ check, status: "COULD NOT RUN", held: "0/0", seconds: 0 })
      continue
    }
    const out = join(dir, `${check}.json`)
    const args = [...passThrough(), "--out", out]
    if (!sharesPage(check)) {
      if (check === "perf-budget" && options.url) args.push("--url", options.url)
      if (options.runs) args.push("--runs", options.runs)
      if (check === "message-sync") args.push("--mode", "prod")
    } else {
      args.push("--url", page.url)
      // The shared page is already started. The child must still hear prod,
      // or a step that needs the dev server's modules reports could-not-run.
      if (page.mode === "dev" || page.mode === "prod") args.push("--mode", page.mode)
    }
    log(`\n=== ${check}`)
    verbose(options, `node ${check}.mjs ${args.join(" ")}`)
    const { code, seconds } = await run(check, args)
    let document = null
    try {
      document = JSON.parse(readFileSync(out, "utf8"))
    } catch {
      // The check did not write a result; its stderr above says why.
    }
    documents[check] = document
    const results = document?.results ?? []
    const counted = results.filter((r) => !r.skipped)
    summary.push({
      check,
      status: verdictOf(code),
      held: `${counted.filter((r) => r.ok).length}/${counted.length}`,
      seconds,
    })
  }
} finally {
  await page?.close()
  rmSync(dir, { recursive: true, force: true })
}

log(`\n${table(summary, ["check", "status", "held", "seconds"])}`)
for (const [check, document] of Object.entries(documents))
  for (const r of document?.results ?? []) {
    if (r.skipped) {
      log(`  ${check} › ${r.name}: ${r.skipped}`)
      continue
    }
    if (!r.ok)
      log(
        `  ${check} › ${r.name} [${[r.engine, r.layout, r.width].filter(Boolean).join(" ")}]: ${r.error ?? r.failures.join("; ")}`,
      )
  }

const status = overallStatus(summary.map((s) => s.status))
const document = {
  check: "run-all",
  ok: status === 0,
  summary,
  checks: documents,
}
const json = `${JSON.stringify(document, null, 2)}\n`
if (options.out) {
  mkdirSync(dirname(resolve(options.out)), { recursive: true })
  writeFileSync(resolve(options.out), json)
  log(`wrote ${resolve(options.out)}`)
} else process.stdout.write(json)
process.exit(status)
