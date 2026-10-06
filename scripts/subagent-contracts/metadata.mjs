/** Publish admitted provider metadata, never provider-owned descriptions or nested values. */
const object = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
const modes = new Set(["read-only", "agent", "agent-full-access"])
const kinds = new Set(["standard", "auto_review", "full_access"])
const capabilities = [
  "resume",
  "list",
  "close",
  "delete",
  "fork",
  "additionalDirectories",
  "subagents",
]

/** Identity must match trusted installed-adapter data; capabilities retain presence only. */
export function initializationMetadata(result, expectedAgent) {
  const agent = result?.agentInfo
  const offered = result?.agentCapabilities?.sessionCapabilities
  if (
    !object(agent) ||
    !object(offered) ||
    !object(expectedAgent) ||
    ![expectedAgent.name, expectedAgent.version].every(
      (value) =>
        typeof value === "string" && value.trim().length > 0 && value.length <= 256,
    )
  )
    throw { code: "initialize_invalid" }
  if (agent.name !== expectedAgent.name || agent.version !== expectedAgent.version)
    throw { code: "metadata_agent_mismatch" }
  const selected = {}
  for (const name of capabilities) {
    if (!Object.hasOwn(offered, name)) continue
    if (!object(offered[name])) throw { code: "initialize_invalid" }
    selected[name] = {}
  }
  return {
    agentInfo: { name: expectedAgent.name, version: expectedAgent.version },
    sessionCapabilities: selected,
  }
}

/** Mode values/kinds use closed vocabularies; display labels and unknown metadata are omitted. */
export function sessionMetadata(result) {
  const initialMode = result?.modes?.currentModeId
  const mode = result?.configOptions?.find((option) => option?.id === "mode")
  if (!modes.has(initialMode) || !object(mode) || mode.currentValue !== initialMode)
    throw { code: "metadata_mode_invalid" }
  const modeConfig = { id: "mode", currentValue: initialMode }
  if (Object.hasOwn(mode, "options")) {
    if (!Array.isArray(mode.options)) throw { code: "metadata_mode_invalid" }
    const seen = new Set()
    modeConfig.options = mode.options.map((option) => {
      if (!object(option) || !modes.has(option.value) || seen.has(option.value))
        throw { code: "metadata_mode_invalid" }
      seen.add(option.value)
      const selected = { value: option.value }
      if (object(option._meta) && Object.hasOwn(option._meta, "kind")) {
        if (!kinds.has(option._meta.kind)) throw { code: "metadata_mode_invalid" }
        selected._meta = { kind: option._meta.kind }
      }
      return selected
    })
  }
  return { initialMode, modeConfig }
}
