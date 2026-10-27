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

/** Streaming deltas may split a marker at any character boundary. */
export function messageText(message) {
  return message.parts
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
}

/** A prior uncertain cleanup remains a block; unrelated failures are not proof. */
export function missingVmRefused(code, lifecycle) {
  return (
    code === "environment_unavailable" ||
    (code === "agent_operation_failed" &&
      lifecycle.phase === "failed" &&
      lifecycle.failure?.code === "cleanup")
  )
}
