/**
 * Command-line plumbing shared by every check: options, diagnostics on
 * stderr, the JSON result on stdout (or `--out`), and the exit status.
 *
 * Exit status: 0 every assertion held; 1 an assertion failed (the result
 * says which); 2 the check could not run (the page was not what the script
 * expects — often the UI mid-change — or the server did not start).
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
Exit: 0 held, 1 an assertion failed, 2 could not run (page not as expected / server).`

/** Parses argv against the common options plus the check's own. */
export function cli({ name, summary, options = {}, help = "", defaults = {} }) {
  const spec = { ...commonOptions, ...options }
  for (const [key, value] of Object.entries(defaults))
    spec[key] = { ...spec[key], default: value }
  let values
  try {
    // `pnpm <script> -- --flag` forwards the `--` itself; drop it.
    const args = process.argv.slice(2).filter((arg, i) => !(i === 0 && arg === "--"))
    ;({ values } = parseArgs({ args, options: spec, allowPositionals: false }))
  } catch (error) {
    process.stderr.write(`${name}: ${error.message}\nRun with --help.\n`)
    process.exit(2)
  }
  if (values.help) {
    process.stdout.write(`${name} — ${summary}\n${help}\n${commonHelp}\n`)
    process.exit(0)
  }
  const list = (value) =>
    String(value ?? "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
  // `--quick` narrows what was not asked for by name to the first of each.
  const given = new Set(
    process.argv.slice(2).map((arg) => arg.replace(/^--(no-)?/, "").split("=")[0]),
  )
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
  }
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
      entry.ok = entry.failures.length === 0 && !entry.error
      results.push(entry)
      const tag = entry.error ? "ERROR" : entry.ok ? "ok   " : "FAIL "
      const where = [entry.engine, entry.layout, entry.width].filter(Boolean).join(" ")
      log(`${tag} ${entry.name}${where ? ` [${where}]` : ""}`)
      for (const failure of entry.failures) log(`        - ${failure}`)
      if (entry.error) log(`        ! ${entry.error}`)
      return entry
    },
    finish(extra = {}) {
      const cannotRun = results.some((r) => r.cannotRun)
      const ok = results.length > 0 && results.every((r) => r.ok)
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
      const passed = results.filter((r) => r.ok).length
      log(
        `${check}: ${passed}/${results.length} held${cannotRun ? " (some could not run)" : ""}`,
      )
      return cannotRun ? 2 : ok ? 0 : 1
    },
  }
}

/** Runs one named step, turning a thrown error into a result instead of a crash. */
export async function attempt(rep, base, body) {
  try {
    const result = await body()
    return rep.add({ ...base, ...result })
  } catch (error) {
    const cannotRun =
      error instanceof CannotRun || /Timeout .*exceeded|waiting for/i.test(error.message)
    return rep.add({
      ...base,
      cannotRun,
      error: `${cannotRun ? "could not run: " : ""}${error.message.split("\n")[0]}`,
    })
  }
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
