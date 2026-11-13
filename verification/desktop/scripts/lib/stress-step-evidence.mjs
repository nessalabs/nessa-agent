/** Retain a failed step's view before cleanup navigates away from it. */
export async function withStepFailureEvidence(action, { failures, capture, cleanup }) {
  let threw = false
  try {
    return await action()
  } catch (error) {
    threw = true
    throw error
  } finally {
    if (threw || failures.length) {
      try {
        await capture()
      } catch {
        // Evidence is best effort; it must not replace the step's failure.
      }
    }
    await cleanup()
  }
}
