#!/usr/bin/env node
/**
 * Fail unless every check this workflow owns ran and passed.
 *
 * GitHub counts a skipped required check as a success. The `changes` job
 * therefore skips compile work only by writing an explicit `false`, and this
 * job is what makes a missing or skipped check visible when that did not
 * happen. A documentation-only change may skip the compile jobs. It may not
 * skip the jobs in `ALWAYS_RUN`, and a failure is a failure either way.
 *
 * The decision is `evaluateChecks`. The process prints that result as JSON on
 * stdout and the failures on stderr, and exits non-zero when the result is not
 * ok. `NEEDS` is `toJSON(needs)` from the workflow.
 */
import { pathToFileURL } from "node:url"

/** Jobs that have no documentation-only guard. A skip here is a missed check. */
export const ALWAYS_RUN = ["changes", "workflows"]

function failure(job, result, reason) {
  return { job, result, reason }
}

function jobResult(outcome) {
  if (!outcome || typeof outcome !== "object") return "missing"
  return typeof outcome.result === "string" ? outcome.result : "missing"
}

/**
 * @param {unknown} needs GitHub's `needs` context for this job.
 * @returns {{ ok: boolean, failures: { job: string, result: string, reason: string }[] }}
 */
export function evaluateChecks(needs) {
  if (needs === null || typeof needs !== "object" || Array.isArray(needs)) {
    return {
      ok: false,
      failures: [failure("(needs)", "invalid", "needs must be an object")],
    }
  }
  if (!Object.hasOwn(needs, "changes")) {
    return {
      ok: false,
      failures: [failure("changes", "missing", "classification did not run")],
    }
  }
  const changes = needs.changes
  const changesResult = jobResult(changes)
  if (changesResult !== "success") {
    return {
      ok: false,
      failures: [failure("changes", changesResult, "classification did not succeed")],
    }
  }
  const outputs = changes && typeof changes === "object" ? changes.outputs : undefined
  const code =
    outputs && typeof outputs === "object" && Object.hasOwn(outputs, "code")
      ? outputs.code
      : undefined
  if (code !== "true" && code !== "false") {
    return {
      ok: false,
      failures: [failure("changes", "success", "classification code is missing")],
    }
  }
  const documentationOnly = code === "false"
  const failures = []
  for (const [job, outcome] of Object.entries(needs)) {
    if (job === "changes") continue
    const result = jobResult(outcome)
    if (result === "success") continue
    if (documentationOnly && result === "skipped" && !ALWAYS_RUN.includes(job)) continue
    failures.push(
      failure(
        job,
        result,
        documentationOnly
          ? "a documentation-only run may skip compile jobs, not this one"
          : "a code change requires this job to run and pass",
      ),
    )
  }
  return { ok: failures.length === 0, failures }
}

function report(result, stdout, stderr) {
  stdout.write(`${JSON.stringify(result)}\n`)
  for (const item of result.failures)
    stderr.write(`${item.job}: ${item.result}: ${item.reason}\n`)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  let needs
  try {
    needs = JSON.parse(process.env.NEEDS ?? "")
  } catch {
    needs = null
  }
  const result = evaluateChecks(needs)
  report(result, process.stdout, process.stderr)
  if (!result.ok) process.exitCode = 1
}
