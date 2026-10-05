#!/usr/bin/env node
/**
 * One signed-out command for the gateway-backed checks (#510). It builds
 * `nessa-server`, starts each check with the scripted agent, and runs it in
 * Chromium and WebKit. stdout is one verdict line. The short summary for a
 * pull request body is printed on stderr and written to `pr-summary.md` in
 * the evidence directory.
 *
 *   pnpm test:e2e:scripted
 *   pnpm test:e2e:scripted -- --mode prod --evidence /tmp/scripted-e2e
 *   pnpm test:e2e:scripted -- --channel bundled
 *
 * `--mode prod` previews a production build for the checks that can run
 * against one. `mcp-apps-gateway` needs the dev server's sandbox meta, so a
 * production run lists it as not run and does not count that as could-not-run.
 * Exit 0 pass, 1 fail, 2 could-not-run.
 *
 * It does not need a harness's node_modules or an owner credential. Live
 * provider checks (`pnpm agent:e2e`) are separate.
 */
import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { parseArgs } from "node:util"
import { fileURLToPath } from "node:url"

import {
  enginesFrom,
  exitOf,
  overallVerdict,
  prSummary,
  relevantLines,
  scriptedCheckArgs,
  verdictLine,
} from "./lib/scripted-evidence.mjs"

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..")
const scripts = join(repoRoot, "verification/desktop/scripts")

const help = `test:e2e:scripted — gateway-backed checks, signed out, Chromium and WebKit

Usage: node verification/desktop/scripts/scripted-e2e.mjs [--mode dev|prod] [--evidence <dir>] [--agent claude|codex] [--channel <name>]

Builds nessa-server, then runs mcp-apps-gateway --scripted, gateway-window
--scripted, and scripted-scenarios. stdout is one JSON verdict line. The
summary is on stderr and in <evidence>/pr-summary.md.

--mode prod skips mcp-apps-gateway (not run: dev server only) and runs the
other two against a production preview. --channel is chrome (installed
Google Chrome) or bundled (Playwright's Chromium), and is forwarded to
each check. Exit 0 pass, 1 fail, 2 could-not-run.
`

function child(args) {
  return new Promise((resolve) => {
    const proc = spawn(process.execPath, args, {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    })
    proc.stdout.on("data", (chunk) => process.stderr.write(chunk))
    proc.stderr.on("data", (chunk) => process.stderr.write(chunk))
    proc.on("error", () => resolve(2))
    proc.on("exit", (code) => resolve(code ?? 2))
  })
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"))
  } catch {
    return null
  }
}

const checks = [
  {
    name: "mcp-apps-gateway",
    args: ["--scripted"],
    skipProd: "not run: dev server only",
  },
  { name: "gateway-window", args: ["--scripted"] },
  { name: "scripted-scenarios", args: [] },
]

let values
try {
  ;({ values } = parseArgs({
    args: process.argv.slice(2).filter((arg, index) => !(index === 0 && arg === "--")),
    options: {
      help: { type: "boolean", short: "h", default: false },
      mode: { type: "string", default: "dev" },
      evidence: { type: "string" },
      agent: { type: "string", default: "claude" },
      channel: { type: "string", default: "chrome" },
    },
    allowPositionals: false,
  }))
} catch (error) {
  process.stderr.write(`test:e2e:scripted: ${error.message}\n`)
  process.exit(2)
}
if (values.help) {
  process.stdout.write(help)
  process.exit(0)
}
if (
  !["dev", "prod"].includes(values.mode) ||
  !["claude", "codex"].includes(values.agent) ||
  !["chrome", "bundled"].includes(values.channel)
) {
  process.stderr.write(
    "test:e2e:scripted: --mode is dev or prod, --agent is claude or codex, and --channel is chrome or bundled\n",
  )
  process.exit(2)
}

const evidence =
  values.evidence ?? `${tmpdir()}/nessa-scripted-e2e-${Date.now().toString(36)}`
mkdirSync(evidence, { recursive: true })

process.stderr.write("building nessa-server…\n")
const build = spawnSync("cargo", ["build", "-p", "nessa-server"], {
  cwd: repoRoot,
  stdio: ["ignore", "pipe", "pipe"],
})
if (build.stdout?.length) process.stderr.write(build.stdout)
if (build.stderr?.length) process.stderr.write(build.stderr)
if (build.status !== 0) {
  const summary = join(evidence, "pr-summary.md")
  const text = prSummary({
    verdict: "could-not-run",
    checks: checks.map((check) => ({
      name: check.name,
      skipped: "not run: the gateway did not build",
    })),
    lines: ["cargo build -p nessa-server failed"],
  })
  writeFileSync(summary, text)
  process.stderr.write(text)
  process.stdout.write(verdictLine({ verdict: "could-not-run", evidence, summary }))
  process.exit(2)
}

const ran = []
for (const check of checks) {
  if (values.mode === "prod" && check.skipProd) {
    process.stderr.write(`${check.name}: ${check.skipProd}\n`)
    ran.push({ name: check.name, skipped: check.skipProd, status: null, engines: {} })
    continue
  }
  const dir = join(evidence, check.name)
  mkdirSync(dir, { recursive: true })
  const out = join(dir, "result.json")
  process.stderr.write(`\n${check.name}\n`)
  const status = await child(
    scriptedCheckArgs({
      script: join(scripts, `${check.name}.mjs`),
      agent: values.agent,
      mode: values.mode,
      channel: values.channel,
      evidence: dir,
      shots: join(dir, "shots"),
      out,
      extra: check.args,
    }),
  )
  const document = readJson(out)
  const fallback = status === 0 ? "pass" : status === 1 ? "fail" : "could-not-run"
  const found = document ? enginesFrom(document) : {}
  const engines = {
    chromium: found.chromium ?? fallback,
    webkit: found.webkit ?? fallback,
  }
  const gatewayLog = existsSync(join(dir, "gateway.log"))
    ? readFileSync(join(dir, "gateway.log"), "utf8")
    : ""
  ran.push({ name: check.name, status, engines, document, gatewayLog })
}

const verdict = overallVerdict(ran)
const lines = ran.flatMap((check) =>
  relevantLines({
    results: check.document?.results ?? [],
    gatewayLog: check.gatewayLog,
    verdict,
  }),
)
const summary = join(evidence, "pr-summary.md")
const text = prSummary({ verdict, checks: ran, lines })
writeFileSync(summary, text)
process.stderr.write(`\n${text}`)
process.stdout.write(verdictLine({ verdict, evidence, summary }))
process.exit(exitOf(verdict))
