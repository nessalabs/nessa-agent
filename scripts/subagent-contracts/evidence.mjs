/** Selected live ACP evidence. This is an evidence checker, not Agent ownership. */
export class EvidenceError extends Error {
  constructor(code) {
    super(code)
    this.code = code
  }
}

const states = new Set([
  "pendingInit",
  "running",
  "completed",
  "errored",
  "shutdown",
  "notFound",
])
const tools = new Set(["spawnAgent", "wait", "closeAgent"])

/**
 * Drop account/auth updates, arbitrary text and unknown frames by construction.
 * Consistently replace opaque provider identities; retain collaboration fields.
 * Delegated prompt and child response text are deliberately omitted unless the
 * response is the controlled CHILD_DONE sentinel. No raw logs are written.
 */
export function selectFrames(records) {
  const identities = new Map()
  const identity = (value) => {
    if (value === null || value === undefined) return value
    if (typeof value !== "string") throw new EvidenceError("invalid_identity")
    if (!identities.has(value)) identities.set(value, `identity-${identities.size + 1}`)
    return identities.get(value)
  }
  const output = []
  for (const { direction, frame } of records) {
    if (direction === "from-agent" && frame.params?.update) {
      const { sessionId, update } = frame.params
      const collaboration = update._meta?.codex?.collaboration
      if (!collaboration || !tools.has(collaboration.tool)) continue
      if (
        frame.jsonrpc !== "2.0" ||
        frame.method !== "session/update" ||
        !["tool_call", "tool_call_update"].includes(update.sessionUpdate) ||
        !["in_progress", "completed"].includes(update.status) ||
        (update.kind !== undefined && update.kind !== "other") ||
        update.title !== collaboration.tool
      )
        throw new EvidenceError("invalid_update")
      const raw = update.rawInput
      if (!raw || !Array.isArray(raw.receiverThreadIds))
        throw new EvidenceError("invalid_collaboration")
      if (!["inProgress", "completed"].includes(raw.status))
        throw new EvidenceError("invalid_status")
      const agentsStates = Object.fromEntries(
        Object.entries(raw.agentsStates ?? {}).map(([id, state]) => {
          if (!states.has(state.status)) throw new EvidenceError("unknown_agent_state")
          return [
            identity(id),
            {
              status: state.status,
              message:
                state.message === null
                  ? null
                  : state.message === "CHILD_DONE"
                    ? state.message
                    : "<omitted>",
            },
          ]
        }),
      )
      output.push({
        direction,
        frame: {
          jsonrpc: frame.jsonrpc,
          method: frame.method,
          params: {
            sessionId: identity(sessionId),
            update: {
              sessionUpdate: update.sessionUpdate,
              toolCallId: identity(update.toolCallId),
              ...(update.kind === undefined ? {} : { kind: update.kind }),
              title: collaboration.tool,
              status: update.status,
              rawInput: {
                prompt: raw.prompt === null ? null : "<delegated task omitted>",
                senderThreadId: identity(raw.senderThreadId),
                receiverThreadIds: raw.receiverThreadIds.map(identity),
                agentsStates,
                model: raw.model === null ? null : "<provider model omitted>",
                reasoningEffort:
                  raw.reasoningEffort === null ? null : "<provider effort omitted>",
                status: raw.status,
              },
              _meta: {
                codex: {
                  collaboration: {
                    tool: collaboration.tool,
                    senderThreadId: identity(collaboration.senderThreadId),
                    receiverThreadIds: collaboration.receiverThreadIds.map(identity),
                  },
                },
              },
            },
          },
        },
      })
    } else if (direction === "from-agent" && frame.result?.stopReason) {
      if (
        !Number.isSafeInteger(frame.id) ||
        !["end_turn", "cancelled", "max_tokens", "max_turn_requests", "refusal"].includes(
          frame.result.stopReason,
        )
      )
        throw new EvidenceError("invalid_terminal")
      output.push({
        direction,
        frame: {
          jsonrpc: "2.0",
          id: frame.id,
          result: { stopReason: frame.result.stopReason },
        },
      })
    }
  }
  return output
}

/** Validate two independently reported identity paths before summarizing a run. */
export function inspectFrames(records) {
  const active = new Map()
  const completed = new Map()
  let parent
  let child
  let stopped
  for (const { direction, frame } of records) {
    if (direction !== "from-agent") throw new EvidenceError("unexpected_direction")
    if (frame.result?.stopReason) {
      if (stopped !== undefined) throw new EvidenceError("duplicate_terminal")
      stopped = frame.result.stopReason
      continue
    }
    if (stopped !== undefined) throw new EvidenceError("activity_after_terminal")
    const { sessionId, update } = frame.params ?? {}
    const meta = update?._meta?.codex?.collaboration
    const raw = update?.rawInput
    if (!meta || !raw || !tools.has(meta.tool))
      throw new EvidenceError("invalid_collaboration")
    if (
      meta.senderThreadId !== raw.senderThreadId ||
      sessionId !== raw.senderThreadId ||
      JSON.stringify(meta.receiverThreadIds) !== JSON.stringify(raw.receiverThreadIds)
    )
      throw new EvidenceError("identity_mismatch")
    if (parent === undefined) parent = sessionId
    else if (parent !== sessionId) throw new EvidenceError("parent_mismatch")
    if (update.title !== meta.tool) throw new EvidenceError("tool_mismatch")
    if (update.status === "in_progress" && raw.status !== "inProgress")
      throw new EvidenceError("status_mismatch")
    if (update.status === "completed" && raw.status !== "completed")
      throw new EvidenceError("status_mismatch")
    const receivers = raw.receiverThreadIds
    if (!Array.isArray(receivers) || new Set(receivers).size !== receivers.length)
      throw new EvidenceError("invalid_receivers")
    const reports = raw.agentsStates
    if (!reports || typeof reports !== "object" || Array.isArray(reports))
      throw new EvidenceError("invalid_agent_states")
    for (const [id, state] of Object.entries(reports)) {
      if (!receivers.includes(id)) throw new EvidenceError("foreign_agent_state")
      if (!states.has(state.status)) throw new EvidenceError("unknown_agent_state")
    }
    if (update.sessionUpdate === "tool_call") {
      if (active.has(update.toolCallId) || completed.has(update.toolCallId))
        throw new EvidenceError("duplicate_call")
      if (
        meta.tool !== "spawnAgent" &&
        (child === undefined || receivers.length !== 1 || receivers[0] !== child)
      )
        throw new EvidenceError("child_mismatch")
      active.set(update.toolCallId, {
        tool: meta.tool,
        sender: raw.senderThreadId,
        receivers: [...receivers],
      })
    } else if (update.sessionUpdate === "tool_call_update") {
      const admitted = active.get(update.toolCallId)
      if (!admitted || admitted.tool !== meta.tool)
        throw new EvidenceError("unstarted_call")
      if (
        admitted.sender !== raw.senderThreadId ||
        ((meta.tool !== "spawnAgent" || admitted.receivers.length !== 0) &&
          JSON.stringify(admitted.receivers) !== JSON.stringify(receivers))
      )
        throw new EvidenceError("call_target_mismatch")
      if (update.status !== "completed") throw new EvidenceError("incomplete_call")
      active.delete(update.toolCallId)
      completed.set(update.toolCallId, meta.tool)
      if (meta.tool === "spawnAgent") {
        if (child !== undefined || receivers.length !== 1 || receivers[0] === parent)
          throw new EvidenceError("invalid_child")
        child = receivers[0]
        if (!Object.hasOwn(reports, child)) throw new EvidenceError("missing_child_state")
      } else {
        if (child === undefined || receivers.length !== 1 || receivers[0] !== child)
          throw new EvidenceError("child_mismatch")
        if (!Object.hasOwn(reports, child)) throw new EvidenceError("missing_child_state")
        if (reports[child].status !== "completed")
          throw new EvidenceError("child_not_completed")
        if (reports[child].message !== "CHILD_DONE")
          throw new EvidenceError("child_result_mismatch")
      }
    } else throw new EvidenceError("unknown_update")
  }
  if (stopped === undefined) throw new EvidenceError("missing_terminal")
  if (active.size !== 0) throw new EvidenceError("unfinished_calls")
  if (stopped !== "end_turn") throw new EvidenceError("unexpected_terminal")
  if (
    JSON.stringify([...completed.values()]) !==
    JSON.stringify(["spawnAgent", "wait", "closeAgent"])
  )
    throw new EvidenceError("missing_native_sequence")
  return {
    parent,
    child,
    tools: [...completed.values()],
    activeCalls: active.size,
    stopReason: stopped,
  }
}
