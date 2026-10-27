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
