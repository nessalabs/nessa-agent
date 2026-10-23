export function failureDetails(error, depth = 0) {
  if (!error || depth > 4) return undefined
  return {
    name: error.name,
    code: error.code,
    message: error.message,
    cause: failureDetails(error.cause, depth + 1),
    errors:
      error instanceof AggregateError
        ? [...error.errors].slice(0, 8).map((item) => failureDetails(item, depth + 1))
        : undefined,
    cleanupError: failureDetails(error.cleanupError, depth + 1),
    settlementError: failureDetails(error.settlementError, depth + 1),
  }
}

export async function finishVerification({
  report,
  root,
  close,
  removeProfile,
  writeReport,
  hasUnconfirmedResources = () => false,
}) {
  report.cleanedTestProfiles = false
  try {
    await close()
    if (hasUnconfirmedResources()) {
      const error = new Error("browser_cleanup_unconfirmed")
      error.code = "browser_cleanup_unconfirmed"
      throw error
    }
    await removeProfile()
    report.cleanedTestProfiles = true
  } catch (error) {
    // Never erase a possibly live browser's profile or lose the primary result.
    report.cleanupError = failureDetails(error)
    report.retainedTestRoot = root
    report.result = "failed"
  }
  await writeReport(report)
  return report
}
