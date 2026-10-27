/** A prior completed message is not evidence for a newly admitted turn. */
export function settledMessage(view, executionId) {
  return (
    view.messages.find(
      (message) =>
        message.executionId === executionId &&
        ["completed", "failed", "cancelled"].includes(message.status),
    ) ?? null
  )
}
