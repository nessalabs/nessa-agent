import { ExperimentFailure } from "./session.mjs"

// One owner of sequential cleanup and preservation of combined failures.
export async function closeResources(resources) {
  const errors = []
  for (const resource of resources) {
    try {
      await resource?.close()
    } catch (error) {
      errors.push(error)
    }
  }
  if (errors.length)
    throw new ExperimentFailure("cleanup_failed", new AggregateError(errors))
}

export async function failAfterCleanup(code, cause, resources) {
  const failure = new ExperimentFailure(code, cause)
  let localCleanupError
  try {
    await closeResources(resources)
  } catch (cleanupError) {
    localCleanupError = cleanupError
  }
  if (localCleanupError || cause?.cleanupOwner) {
    failure.cleanupError = localCleanupError ?? cause.cleanupError
    // Preserve nested initialization ownership without implicitly retrying it.
    failure.cleanupOwner = {
      close: () => closeResources([cause?.cleanupOwner, ...resources]),
    }
  }
  throw failure
}
