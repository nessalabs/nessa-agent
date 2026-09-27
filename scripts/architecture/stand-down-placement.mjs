import { normalizedPath } from "./rust-boundaries.mjs"

/** The developer setup script, whose functions other processes call directly. */
const DEV_AGENT_CONFIG = "scripts/dev-agent-config.mjs"

/**
 * This script ends its own process in exactly one place: its entry point.
 *
 * `publish` and everything it reaches are called by `node --test` in the
 * caller's own process. A `process.exit` anywhere below the entry point
 * therefore ends the test run itself, at the first test to reach that line,
 * and `node --test` reports the tests that did run as passing and exits 0. A
 * file of twenty tests reports one, CI is green, and nineteen guards over a
 * script that rewrites a developer's files never ran.
 *
 * Exiting from under the configuration lock is the second cost: it skips the
 * `finally` that releases it, leaving a lock file for the next run to report
 * as somebody else's.
 *
 * So a stand-down throws and the entry point turns it back into the status a
 * stand-down carries. That rule is correct and, without this, unguarded: the
 * regression test for it ends its own process inside `assert.throws`, so it
 * cannot observe the very failure it guards, and nothing else asserts a test
 * count. Checked in the source, because the failure being prevented is a test
 * run that ends early and reports success.
 */
export function standDownPlacementViolations(path, source) {
  if (normalizedPath(path) !== DEV_AGENT_CONFIG) return []

  // The entry point, where ending the process is the whole point: everything
  // from the guard that recognises direct invocation to the end of the file.
  //
  // Anchored on that `if` rather than on `import.meta.url` alone, because the
  // script also resolves its own directory from `import.meta.url` near the top
  // of the file — matching the first mention would treat almost the whole file
  // as the entry point and this rule would pass on anything.
  const guard = source.search(/^if \(.*\bimport\.meta\.url\b.*\) \{$/m)
  const body = guard === -1 ? source : source.slice(0, guard)

  const failures = []
  if (guard === -1) {
    failures.push(
      "this script must keep its one process-ending block behind an import.meta.url guard, so importing it runs nothing",
    )
  }
  if (/\bprocess\.exit\b/.test(body)) {
    failures.push(
      "process.exit belongs only in this script's entry point; below it, a stand-down ends the node --test run that called it and its remaining tests are reported as passing",
    )
  }
  return failures
}
