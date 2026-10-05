/**
 * The signed-out scripted run's verdict and the short summary a pull request
 * shows. The page's console and request lines stay owned by `browser.mjs`
 * (`recordFailedRequest`); this reads the `failures` and `harmless` arrays
 * the checks already record. It does not classify them again.
 */
import { mkdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

/** A page line `browser.mjs` recorded: a console error, a page error, or a failed request. */
const PAGE_LINE = /^(?:pageerror: |console\.error: |requestfailed: )/

/**
 * One JSON line for stdout: the verdict, the evidence directory, and the
 * summary file. Diagnostics stay off this line.
 */
export function verdictLine({ verdict, evidence, summary }) {
  return `${JSON.stringify({ verdict, evidence, summary })}\n`
}

/** Exit status for a verdict: 0 pass, 1 fail, 2 could not run. */
export function exitOf(verdict) {
  if (verdict === "pass") return 0
  if (verdict === "fail") return 1
  return 2
}

/**
 * The run's verdict from its checks. A check with `skipped` set did not run
 * and is not "could not run" — a production run leaves the dev-server-only
 * check out and says so. A crash (`status` other than 0 or 1) with nothing
 * failed is could-not-run. A failed assertion wins over that.
 */
export function overallVerdict(checks) {
  const ran = checks.filter((check) => !check.skipped)
  if (ran.length === 0) return "could-not-run"
  if (ran.some((check) => check.status === 1)) return "fail"
  if (ran.some((check) => check.status !== 0)) return "could-not-run"
  return "pass"
}

/** Per-engine status from one check's JSON: pass, fail, or could-not-run. */
export function enginesFrom(document) {
  const rank = { pass: 0, "could-not-run": 1, fail: 2 }
  const worse = (left, right) => (rank[left] >= rank[right] ? left : right)
  const engines = {}
  for (const result of document?.results ?? []) {
    if (!result.engine) continue
    const status = result.cannotRun ? "could-not-run" : result.ok ? "pass" : "fail"
    engines[result.engine] = engines[result.engine]
      ? worse(engines[result.engine], status)
      : status
  }
  return engines
}

/**
 * Lines worth putting in the summary: each result's assertion failures and
 * error, the page's console and request lines (including ones the check
 * labelled harmless), and, when the run did not pass, the tail of the
 * gateway log.
 */
export function relevantLines({ results = [], gatewayLog = "", verdict = "pass" }) {
  const lines = []
  for (const result of results) {
    const where = [result.name, result.engine].filter(Boolean).join(" ")
    for (const failure of result.failures ?? []) lines.push(`${where}: ${failure}`)
    if (result.error) lines.push(`${where}: ${result.error}`)
    for (const line of result.harmless ?? [])
      if (PAGE_LINE.test(line)) lines.push(`${where}: ${line}`)
  }
  if (verdict !== "pass" && gatewayLog.trim())
    lines.push(...gatewayLog.trim().split("\n").slice(-20))
  return lines
}

/** The summary a pull request body pastes: verdict, checks per engine, relevant lines. */
export function prSummary({ verdict, checks, lines }) {
  const rows = checks.map((check) => {
    const cell = (engine) => check.skipped ?? check.engines?.[engine] ?? "could-not-run"
    return `| ${check.name} | ${cell("chromium")} | ${cell("webkit")} |`
  })
  const logged = lines.length > 0 ? lines.map((line) => `- ${line}`).join("\n") : "- none"
  return [
    `Verdict: ${verdict}`,
    "",
    "| Check | Chromium | WebKit |",
    "| --- | --- | --- |",
    ...rows,
    "",
    "Relevant log lines:",
    "",
    logged,
    "",
  ].join("\n")
}

/** Writes `view` as pretty JSON at `directory`/`name`, creating the directory. */
export function writeView(directory, name, view) {
  const path = join(directory, name)
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, `${JSON.stringify(view, null, 2)}\n`)
  return path
}
