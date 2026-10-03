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

/**
 * The option that allows `permission` once, when it asks about a call to one
 * of `server`'s tools — and `null` for anything else the agent asks, so a run
 * never allows a shell command, an edit, or a standing approval. The call is
 * found in the view by its identity, since a permission's own name can be a
 * kind (`execute`) rather than the tool.
 */
export function allowOnce(view, permission, server) {
  const tool = (view?.tools ?? []).find(
    (each) =>
      each.executionId === permission.executionId && each.toolId === permission.toolId,
  )
  if (tool?.mcp?.server !== server) return null
  return permission.options.find((option) => /^allow[-_]once$/i.test(option.id)) ?? null
}

/** A pending permission's identity in a view. */
export const permissionKey = ({ executionId, permissionId }) =>
  `${executionId}:${permissionId}`

/**
 * What to do about the view's open permissions, given those already answered
 * (keys `permissionKey`, never answered twice): the ones to allow,
 * each with its allow-once option, and the ones declined because they are not
 * a call to `server`'s tools — which the run reports rather than answers.
 */
export function permissionDecisions(view, answered, server) {
  const allow = []
  const declined = []
  for (const permission of view?.permissions ?? []) {
    const key = permissionKey(permission)
    if (answered.has(key)) continue
    const option = allowOnce(view, permission, server)
    if (option) allow.push({ key, permission, option })
    else declined.push({ key, permission })
  }
  return { allow, declined }
}

/** The view's tools whose MCP identity names `server`. */
export function viewTools(view, server) {
  return (view?.tools ?? []).filter((tool) => tool.mcp?.server === server)
}

/**
 * The MCP servers the gateway gave the harness when it opened or reopened a
 * session (`session/new`, `session/load`, `session/resume`), as it sent them.
 * Under ADR 344 each is a stand-in — the gateway's own executable running
 * `mcp-relay` — never the configured server itself.
 */
export function givenServers(records) {
  return records.flatMap(({ direction, frame }) => {
    if (
      direction !== "to-agent" ||
      !["session/new", "session/load", "session/resume"].includes(frame?.method)
    )
      return []
    return (frame.params?.mcpServers ?? []).map(({ name, command, args }) => ({
      name,
      command,
      args,
    }))
  })
}
