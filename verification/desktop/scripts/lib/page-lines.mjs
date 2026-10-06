/**
 * One owner for a page's console errors and failed requests (#494).
 *
 * A bound reporter is process-global: `run.mjs` binds it, and every
 * `openPage` watches without a `rep` argument. A result takes the lines of
 * the page opened in the async work that produced it, so two workers with
 * two pages open attribute each line to its own result. A result that could
 * not run, and the close path's own result, take none. Close reports
 * whatever that page still holds as a `console` result. A line is taken by
 * splice, so a second report of it has nothing left to take.
 *
 * The ordering is `verification/desktop/page-lines.md`.
 */
import { AsyncLocalStorage } from "node:async_hooks"

/** Marks a result that must not take lines, because it is the report of them. */
export const skipLineDrain = Symbol("skipLineDrain")

/** @type {null | { add(result: object): unknown }} */
let reporter = null

/** The page whose lines the current async work produces. */
const currentPage = new AsyncLocalStorage()

/** `rep` receives every later page's lines. One reporter per process. */
export function bindReporter(rep) {
  reporter = rep
}

/** Whether `openPage` should watch for a reporter. */
export function reporterBound() {
  return reporter !== null
}

/**
 * The arrays a page pushes lines onto, and the ways a script reclassifies a
 * line it already knows is not a failure. `noteHarmless` is a pattern that
 * also covers a line still to come. `reclassifyHeld` moves lines already
 * held, once a step has learned which of them are noise.
 */
export function watchLines() {
  const errors = []
  const harmless = []
  /** @type {RegExp[]} */
  const patterns = []
  const matches = (line) => patterns.some((pattern) => pattern.test(line))
  return {
    errors,
    harmless,
    keep(line, asHarmless) {
      if (asHarmless || matches(line)) harmless.push(line)
      else errors.push(line)
    },
    noteHarmless(pattern) {
      patterns.push(pattern)
      const stay = []
      for (const line of errors.splice(0, errors.length)) {
        if (pattern.test(line)) harmless.push(line)
        else stay.push(line)
      }
      errors.push(...stay)
    },
    /**
     * Move held errors `keep` accepts onto `harmless`, as `asHarmless`
     * rewrites them. A line `keep` declines stays an error.
     *
     * @param {(line: string) => boolean} keep
     * @param {(line: string) => string} asHarmless
     */
    reclassifyHeld(keep, asHarmless) {
      const stay = []
      for (const line of errors.splice(0, errors.length)) {
        if (keep(line)) harmless.push(asHarmless(line))
        else stay.push(line)
      }
      errors.push(...stay)
    },
  }
}

/**
 * Report `errors` and `harmless` once, for a page that never became a step's
 * page (it failed while opening). They are taken off the arrays.
 *
 * @param {object} base
 * @param {string[]} errors
 * @param {string[]} harmless
 */
export function reportDetached(base, errors, harmless) {
  if (!reporter) return
  const takenErrors = errors.splice(0, errors.length)
  const takenHarmless = harmless.splice(0, harmless.length)
  if (takenErrors.length === 0 && takenHarmless.length === 0) return
  reporter.add({
    ...base,
    name: "console",
    ...(takenErrors.length ? { failures: takenErrors } : {}),
    ...(takenHarmless.length ? { harmless: takenHarmless } : {}),
    [skipLineDrain]: true,
  })
}

/**
 * Watch `opened` until it closes. `base` is the engine and layout a late
 * `console` result keeps. The async work that opens the page is the one
 * whose later results take its lines. Does nothing when no reporter is bound.
 *
 * @param {{ errors: string[], harmless: string[], close(): Promise<unknown> }} opened
 * @param {object} base
 */
export function attachLines(opened, base) {
  if (!reporter) return
  const entry = { opened, base }
  currentPage.enterWith(entry)
  const original = opened.close
  let closed = false
  opened.close = async () => {
    if (closed) return
    closed = true
    const errors = opened.errors.splice(0, opened.errors.length)
    const harmless = opened.harmless.splice(0, opened.harmless.length)
    try {
      if ((errors.length > 0 || harmless.length > 0) && reporter) {
        reporter.add({
          ...base,
          name: "console",
          ...(errors.length ? { failures: errors } : {}),
          ...(harmless.length ? { harmless } : {}),
          [skipLineDrain]: true,
        })
      }
    } finally {
      await original()
    }
  }
}

/**
 * Move the current work's page lines onto `result`. A result that could not
 * run, and one marked [`skipLineDrain`], is returned unchanged and leaves
 * the lines for a later result of this work, or for close. Work that opened
 * no page takes nothing, including while another page is open.
 *
 * @param {object} result
 */
export function applyLines(result) {
  if (result[skipLineDrain] || result.cannotRun) return result
  const entry = currentPage.getStore()
  if (!entry) return result
  const errors = entry.opened.errors.splice(0, entry.opened.errors.length)
  const harmless = entry.opened.harmless.splice(0, entry.opened.harmless.length)
  if (errors.length === 0 && harmless.length === 0) return result
  const next = { ...result }
  if (errors.length > 0 || Object.hasOwn(result, "failures"))
    next.failures = [...(result.failures ?? []), ...errors]
  if (harmless.length > 0) next.harmless = [...(result.harmless ?? []), ...harmless]
  return next
}
