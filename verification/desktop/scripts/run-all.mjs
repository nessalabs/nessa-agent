#!/usr/bin/env node
/**
 * Runs every desktop check and summarises them. The functional checks share
 * one page (the dev server by default); perf-budget builds and previews
 * production itself, as its budget requires. Takes minutes — run it before
 * handing off UI work, not on every change; while iterating, run the check
 * that covers the change with `--quick` (one engine, layout and size).
 */
import { spawn } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { cli, log, table, verbose } from "./lib/cli.mjs"
import { target } from "./lib/server.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const functional = ["smoke", "focus", "drag", "responsive", "safe-area"]

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

  --only <list>    Checks to run (default: ${[...functional, "perf-budget"].join(", ")}).
  --skip-perf      Leave out perf-budget (the production build and its runs).
  --runs <n>       Passed to perf-budget.

--url / --mode apply to the functional checks; perf-budget always measures a
production build unless --url is given. --engine and --layout, when given,
are passed to every check; otherwise each uses its own default. Each
check's JSON is collected into one document on stdout (or --out).`,
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

function run(check, args) {
  return new Promise((ok) => {
    const started = Date.now()
    const proc = spawn(process.execPath, [join(here, `${check}.mjs`), ...args], {
      stdio: ["ignore", "ignore", "pipe"],
    })
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
    proc.on("exit", (code) =>
      ok({ code, seconds: Math.round((Date.now() - started) / 1000) }),
    )
  })
}

const chosen = options.only
  ? options.list(options.only)
  : [...functional, ...(options["skip-perf"] ? [] : ["perf-budget"])]
const dir = mkdtempSync(join(tmpdir(), "nessa-desktop-verify-all-"))
const summary = []
const documents = {}
let page
try {
  if (chosen.some((c) => functional.includes(c))) page = await target(options)
  for (const check of chosen) {
    const out = join(dir, `${check}.json`)
    const args = [...passThrough(), "--out", out]
    if (check === "perf-budget") {
      if (options.url) args.push("--url", options.url)
      if (options.runs) args.push("--runs", options.runs)
    } else args.push("--url", page.url)
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
    summary.push({
      check,
      status: code === 0 ? "held" : code === 1 ? "FAILED" : "COULD NOT RUN",
      held: `${results.filter((r) => r.ok).length}/${results.length}`,
      seconds,
    })
  }
} finally {
  await page?.close()
  rmSync(dir, { recursive: true, force: true })
}

log(`\n${table(summary, ["check", "status", "held", "seconds"])}`)
for (const [check, document] of Object.entries(documents))
  for (const r of document?.results ?? [])
    if (!r.ok)
      log(
        `  ${check} › ${r.name} [${[r.engine, r.layout, r.width].filter(Boolean).join(" ")}]: ${r.error ?? r.failures.join("; ")}`,
      )

const all = {
  check: "run-all",
  ok: summary.every((s) => s.status === "held"),
  summary,
  checks: documents,
}
const json = `${JSON.stringify(all, null, 2)}\n`
if (options.out) {
  mkdirSync(dirname(resolve(options.out)), { recursive: true })
  writeFileSync(resolve(options.out), json)
  log(`wrote ${resolve(options.out)}`)
} else process.stdout.write(json)
process.exit(summary.some((s) => s.status === "COULD NOT RUN") ? 2 : all.ok ? 0 : 1)
