import { resultOfThrown } from "./cli.mjs"

/** Retain a failed step's view before cleanup navigates away from it. */
export async function withStepFailureEvidence(action, { failures, capture, cleanup }) {
  let outcome
  try {
    outcome = { kind: "returned", value: await action() }
  } catch (error) {
    outcome = { kind: "threw", error }
  }
  if (outcome.kind === "threw" || failures.length) {
    try {
      await capture()
    } catch {
      // Evidence is best effort; it must not replace the step's failure.
    }
  }
  let cleanupOutcome
  try {
    await cleanup()
    cleanupOutcome = { kind: "returned" }
  } catch (error) {
    cleanupOutcome = { kind: "threw", error }
  }
  if (cleanupOutcome.kind === "threw") {
    if (outcome.kind === "threw")
      throw new AggregateError(
        [outcome.error, cleanupOutcome.error],
        "the step and its cleanup failed",
        { cause: outcome.error },
      )
    throw cleanupOutcome.error
  }
  if (outcome.kind === "threw") throw outcome.error
  return outcome.value
}

/** Report one step through the CLI's failure owner, including falsy throws. */
export async function reportStressStep(rep, base, step, action) {
  const at = Date.now()
  const failures = []
  let outcome
  try {
    await action(failures)
    outcome = { kind: "returned" }
  } catch (error) {
    outcome = { kind: "threw", error }
  }
  const ms = Date.now() - at
  return rep.add({
    ...base,
    ms,
    seen: step,
    failures,
    ...(outcome.kind === "threw" ? resultOfThrown({}, outcome.error) : {}),
  })
}
