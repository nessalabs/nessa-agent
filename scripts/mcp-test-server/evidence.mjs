/**
 * Read a live check's recordings: the ACP frames an agent sent about tool
 * calls, and what the gateway's view then said about the same calls. Pure, so
 * the check's verdicts are tested without a gateway or an agent.
 */

/** Parsed recorder lines, skipping any that are not JSON. */
export function parseRecording(text) {
  return text
    .split("\n")
    .filter((line) => line.trim())
    .flatMap((line) => {
      try {
        return [JSON.parse(line)]
      } catch {
        return []
      }
    })
}

/** The `tool_call` and `tool_call_update` updates the agent sent, in order. */
export function toolFrames(records) {
  return records.flatMap(({ direction, frame }) => {
    const update = frame?.params?.update
    if (
      direction !== "from-agent" ||
      frame?.method !== "session/update" ||
      !["tool_call", "tool_call_update"].includes(update?.sessionUpdate)
    )
      return []
    return [update]
  })
}

/**
 * Every place a `ui://` resource or a `resourceUri` appears in what the agent
 * sent, as a path into its frame. Empty means the tool's UI never arrived.
 */
export function uiMentions(records) {
  const found = []
  const walk = (value, path) => {
    if (typeof value === "string") {
      if (value.includes("ui://")) found.push(path)
    } else if (value && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        const next = `${path}.${key}`
        if (key === "resourceUri") found.push(next)
        walk(child, next)
      }
    }
  }
  records.forEach(({ direction, frame }, index) => {
    if (direction === "from-agent") walk(frame, `[${index}]`)
  })
  return found
}

/** The shape of one frame, small enough to compare across harnesses. */
export function frameShape(update) {
  const content = Array.isArray(update.content)
    ? update.content.map((item) =>
        item?.type === "content" ? `content:${item.content?.type}` : String(item?.type),
      )
    : update.content === undefined
      ? undefined
      : "invalid"
  return {
    toolCallId: update.toolCallId,
    sessionUpdate: update.sessionUpdate,
    title: update.title,
    kind: update.kind,
    status: update.status,
    meta: update._meta,
    rawInput: update.rawInput,
    rawOutput: update.rawOutput,
    content,
  }
}

/** The view's tools whose MCP identity names `server`, keyed by tool name. */
export function viewTools(view, server) {
  return (view?.tools ?? []).filter((tool) => tool.mcp?.server === server)
}
