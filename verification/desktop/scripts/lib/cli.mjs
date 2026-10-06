/**
 * Command-line plumbing shared by every check: options, diagnostics on
 * stderr, the JSON result on stdout (or `--out`), and the exit status.
 *
 * Exit status (`statusOf`): 0 every assertion held; 1 an assertion failed
 * (the result says which), whatever else could not run; 2 nothing failed but
 * something could not run — the server did not start, a browser is missing,
 * or what a step needs to begin was not on the page.
 */
import { mkdirSync, writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { parseArgs } from "node:util"

/** Options every check takes. */
export const commonOptions = {
  help: { type: "boolean", short: "h", default: false },
  url: { type: "string" },
  mode: { type: "string", default: "dev" },
  engine: { type: "string", default: "chromium" },
  channel: { type: "string", default: "chrome" },
  layout: { type: "string", default: "columns,sidebar" },
  headed: { type: "boolean", default: false },
  out: { type: "string" },
  shots: { type: "string" },
  verbose: { type: "boolean", short: "v", default: false },
  quick: { type: "boolean", default: false },
}

export const commonHelp = `
Common options:
  --url <url>          Page to test. Skips starting a server.
  --mode dev|prod      dev: reuse or start the Vite dev server on 127.0.0.1:1438.
                       prod: vite build into a temp dir and vite preview on a free port.
                       (default: dev; perf-budget defaults to prod)
  --engine <list>      chromium, webkit, or chromium,webkit (default varies per check)
  --channel <name>     Chromium channel: chrome (installed Google Chrome) or bundled
                       (Playwright's Chromium). Default: chrome.
  --layout <list>      columns, sidebar, or columns,sidebar (default: both)
  --headed             Show the browser.
  --out <file>         Write the JSON result there instead of stdout.
  --shots <dir>        Save screenshots there (checks that take them).
  --quick              While iterating: one engine, one layout and one size —
                       the first of each check's defaults, unless given — so a
                       check answers in seconds. Not evidence for a hand-off:
                       run the whole set (run-all) once the change holds.
  -v, --verbose        Echo server output and per-step detail to stderr.
  -h, --help           This help.

Output: JSON on stdout (or --out); progress and tables on stderr.
Exit: 0 held; 1 an assertion failed (whatever else could not run); 2 nothing
failed but something could not run (server down, browser missing, what a step
needs to begin not on the page).`

/**
 * Parses `argv` (without node and the script) against the common options
 * plus the check's own. Pure: returns the options, `{ help: true, ... }` when
 * help was asked for, or throws a `UsageError`.
 */
export function parseOptions({ options = {}, defaults = {} }, argv) {
  const spec = { ...commonOptions, ...options }
  for (const [key, value] of Object.entries(defaults))
    spec[key] = { ...spec[key], default: value }
  // `pnpm <script> -- --flag` forwards the `--` itself; drop it.
  const args = argv.filter((arg, i) => !(i === 0 && arg === "--"))
  let values
  try {
    ;({ values } = parseArgs({ args, options: spec, allowPositionals: false }))
  } catch (error) {
    throw new UsageError(error.message)
  }
  const list = (value) =>
    String(value ?? "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
  // `--quick` narrows what was not asked for by name to the first of each.
  const given = new Set(args.map((arg) => arg.replace(/^--(no-)?/, "").split("=")[0]))
  const narrowed = (key) => {
    const all = list(values[key])
    return values.quick && !given.has(key) ? all.slice(0, 1) : all
  }
  return {
    ...values,
    engines: narrowed("engine"),
    layouts: narrowed("layout"),
    /** A list option of the check's own, narrowed by `--quick` as the common ones are. */
    choices: narrowed,
    list,
    /** Whether an option was given on the command line rather than defaulted. */
    given: (key) => given.has(key),
  }
}

/** Parses the process's arguments; prints help or a usage error and exits (0, or 2). */
export function cli(meta) {
  let options
  try {
    options = parseOptions(meta, process.argv.slice(2))
  } catch (error) {
    if (!(error instanceof UsageError)) throw error
    process.stderr.write(`${meta.name}: ${error.message}\nRun with --help.\n`)
    process.exit(2)
  }
  if (options.help) {
    process.stdout.write(
      `${meta.name} — ${meta.summary}\n${meta.help ?? ""}\n${commonHelp}\n`,
    )
    process.exit(0)
  }
  return options
}

/**
 * A production run leaves a dev-server check out with this status. It is not
 * "could not run": the production page was never going to host it.
 */
export const DEV_SERVER_ONLY = "not run: dev server only"

/**
 * Functional checks that need a page only the dev server serves. `run-all
 * --mode prod` leaves them out (`checksUnder`) instead of running them.
 */
export const devServerOnlyChecks = [
  "committed-transcript",
  "app-review",
  "gateway-states",
]

/**
 * Steps, inside a check that otherwise runs on a production preview, that
 * read the dev server's source modules.
 */
export const devServerOnlySteps = {
  drag: ["boundary-jitter"],
  widgets: ["off-missing"],
  "mcp-apps": ["departures", "departures-back"],
}

/**
 * The checks a run spawns, and the ones a production run leaves out.
 * Left-out checks stay in the report and are not could-not-run.
 */
export function checksUnder(mode, checks, devOnly = devServerOnlyChecks) {
  if (mode !== "prod") return { run: [...checks], leftOut: [] }
  return {
    run: checks.filter((check) => !devOnly.includes(check)),
    leftOut: checks.filter((check) => devOnly.includes(check)),
  }
}

/**
 * Records `name` as left out of a production run and reports whether it did.
 * `devOnly` is that check's own list (`devServerOnlySteps`).
 */
export function recordIfLeftOut(rep, mode, name, devOnly, fields) {
  if (mode !== "prod" || !devOnly.includes(name)) return false
  rep.add({ ...fields, name, skipped: DEV_SERVER_ONLY, failures: [] })
  return true
}

/**
 * The exit status a set of results earns: `1` when any contract broke — a
 * failure, or an error that was not "could not run" — whatever else could
 * not run; else `2` when anything could not run, or nothing ran at all; else
 * `0`. A product failure is never reported as "could not run". A result with
 * `skipped` set did not run and counts as neither.
 */
export function statusOf(results) {
  const counted = results.filter((result) => !result.skipped)
  const broke = counted.some(
    (r) => !r.cannotRun && ((r.failures ?? []).length > 0 || Boolean(r.error)),
  )
  if (broke) return 1
  if (counted.length === 0 || counted.some((r) => r.cannotRun)) return 2
  return 0
}

/**
 * What a check's exit code says, for `run-all`'s summary: 0 held, 1 failed,
 * 2 could not run; anything else — a crash, a signal — failed, since it did
 * not say it could not run.
 */
export function verdictOf(code) {
  return code === 0 ? "held" : code === 2 ? "COULD NOT RUN" : "FAILED"
}

/**
 * `run-all`'s exit status from its checks' verdicts, by the same rule as `statusOf`.
 * `DEV_SERVER_ONLY` is a check the production run left out, not one that could not run.
 */
export function overallStatus(verdicts) {
  const counted = verdicts.filter((verdict) => verdict !== DEV_SERVER_ONLY)
  if (counted.some((v) => v === "FAILED")) return 1
  if (counted.length === 0 || counted.some((v) => v === "COULD NOT RUN")) return 2
  return 0
}

/** Diagnostics: stderr only, so stdout stays data. */
export const log = (...parts) => process.stderr.write(`${parts.join(" ")}\n`)
export const verbose = (options, ...parts) => {
  if (options.verbose) log(...parts)
}

/** A failure that means "could not run", not "the contract broke". */
export class CannotRun extends Error {
  constructor(message) {
    super(message)
    this.name = "CannotRun"
  }
}

/** Arguments a check was not written for: it could not run, and says why. */
export class UsageError extends CannotRun {
  constructor(message) {
    super(message)
    this.name = "UsageError"
  }
}

/**
 * The names `--only` chose among `available`, in the order given, or all of
 * them. A name that is not one of them is a `UsageError`, not a silent skip.
 */
export function chosen(only, available, list) {
  if (!only) return [...available]
  const names = list(only)
  const unknown = names.filter((name) => !available.includes(name))
  if (unknown.length > 0)
    throw new UsageError(
      `--only: no check named ${unknown.join(", ")} (available: ${available.join(", ")})`,
    )
  return names
}

/**
 * Collects named results, each with its own failures, and finishes the
 * process with the JSON document and the right exit status.
 */
export function report(check, options) {
  const results = []
  const started = new Date().toISOString()
  return {
    results,
    /** Adds a result; `failures` are the assertion messages that did not hold. */
    add(result) {
      const entry = { ok: (result.failures ?? []).length === 0, failures: [], ...result }
      entry.ok = entry.failures.length === 0 && !entry.error && !entry.skipped
      results.push(entry)
      const tag = entry.skipped
        ? "skip "
        : entry.error
          ? "ERROR"
          : entry.ok
            ? "ok   "
            : "FAIL "
      const where = [entry.engine, entry.layout, entry.width].filter(Boolean).join(" ")
      log(`${tag} ${entry.name}${where ? ` [${where}]` : ""}`)
      for (const failure of entry.failures) log(`        - ${failure}`)
      if (entry.skipped) log(`        - ${entry.skipped}`)
      if (entry.error) log(`        ! ${entry.error}`)
      return entry
    },
    finish(extra = {}) {
      const counted = results.filter((r) => !r.skipped)
      const cannotRun = counted.some((r) => r.cannotRun)
      const status = statusOf(results)
      const ok = status === 0
      const document = {
        check,
        ok,
        started,
        finished: new Date().toISOString(),
        target: extra.target,
        ...extra,
        results,
      }
      const json = `${JSON.stringify(document, null, 2)}\n`
      if (options.out) {
        const path = resolve(options.out)
        mkdirSync(dirname(path), { recursive: true })
        writeFileSync(path, json)
        log(`wrote ${path}`)
      } else {
        process.stdout.write(json)
      }
      const passed = counted.filter((r) => r.ok).length
      const leftOut = results.length - counted.length
      log(
        `${check}: ${passed}/${counted.length} held${leftOut ? `, ${leftOut} ${DEV_SERVER_ONLY}` : ""}${cannotRun ? " (some could not run)" : ""}`,
      )
      return status
    },
  }
}

/**
 * Runs one named step, turning a thrown error into a result instead of a
 * crash. Only a `CannotRun` — the page, browser or server not what the step
 * needs to begin — is "could not run"; anything else thrown, a timeout
 * waiting for the product to do something included, is the step failing.
 */
export async function attempt(rep, base, body) {
  try {
    const result = await body()
    return rep.add({ ...base, ...result })
  } catch (error) {
    return rep.add(resultOfThrown(base, error))
  }
}

/**
 * The result a step that threw earns (`attempt`). The result keeps only the
 * error's first line. A throw that is not a `CannotRun` is a fault, so its
 * stack goes to stderr here, and nowhere else (#419).
 */
export function resultOfThrown(base, error) {
  const cannotRun = error instanceof CannotRun
  if (!cannotRun) log(error?.stack ?? String(error?.message ?? error))
  const message = String(error?.message ?? error).split("\n")[0]
  return { ...base, cannotRun, error: `${cannotRun ? "could not run: " : ""}${message}` }
}

/** Formats rows as an aligned text table for stderr. */
export function table(rows, columns) {
  const widths = columns.map((c) =>
    Math.max(c.length, ...rows.map((r) => String(r[c] ?? "").length)),
  )
  const line = (cells) =>
    cells.map((cell, i) => String(cell ?? "").padEnd(widths[i])).join("  ")
  return [
    line(columns),
    line(widths.map((w) => "-".repeat(w))),
    ...rows.map((r) => line(columns.map((c) => r[c]))),
  ].join("\n")
}
