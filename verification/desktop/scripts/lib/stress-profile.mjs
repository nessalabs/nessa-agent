import { CannotRun } from "./cli.mjs"
import { acpRecords } from "./acp-recording.mjs"
import catalog from "../../../../crates/nessa-sdk/data/models.json" with { type: "json" }
import { MODELS } from "../../../../scripts/mcp-test-server/local-gateway.mjs"

/** Explicit workload requirements, not a copy of provider availability policy. */
export function stressProfile(options) {
  const scenario = options.accessScenario ?? "full"
  if (!["full", "ask-only"].includes(scenario))
    throw new Error(`unknown stress access scenario: ${scenario}`)
  const model = options.model ?? MODELS[options.agent]
  const provider = { claude: "anthropic", codex: "openai" }[options.agent]
  if (
    !provider ||
    !catalog.models.some((row) => row.modelId === model && row.provider === provider)
  )
    throw new Error(`model ${model} is not in ${options.agent}'s catalog`)
  return { agent: options.agent, model, scenario }
}

class StressProfileMismatch extends Error {}

/** Reject an incompatible exact fixture; do not fall back or rewrite the workload. */
export function requireStressCapabilities(profile, model) {
  if (model?.modelId !== profile.model)
    throw new StressProfileMismatch(
      `stress fixture model differs from requested ${profile.model}`,
    )
  const offered = model.approvalModes?.map((choice) => choice.id)
  const required = profile.scenario === "full" ? ["ask", "auto", "full"] : ["ask"]
  if (
    !offered ||
    offered.length !== required.length ||
    !required.every((id) => offered.includes(id))
  )
    throw new StressProfileMismatch(
      `${profile.scenario} scenario is incompatible with ${profile.model}: advertised ${JSON.stringify(offered)}, required ${JSON.stringify(required)}`,
    )
  return { ...profile, advertisedModes: [...offered], requiredModes: required }
}

/** Only pre-workload incompatibility is unavailable; later view disagreement is a failure. */
export function selectStressFixture(profile, agents) {
  const model = agents
    .find((row) => row.agent === profile.agent)
    ?.models.find((row) => row.modelId === profile.model)
  try {
    return requireStressCapabilities(profile, model)
  } catch (error) {
    if (error instanceof StressProfileMismatch) throw new CannotRun(error.message)
    throw error
  }
}

/** Retain the full recording prefix; a refused pick must never configure elevated access. */
export function elevatedAccessAfter(before, after) {
  if (!before || !after.startsWith(before))
    throw new Error("missing or replaced ACP recording")
  acpRecords(before)
  return acpRecords(after.slice(before.length)).some(({ direction, frame }) => {
    if (
      direction !== "to-agent" ||
      !["session/set_config_option", "session/set_mode"].includes(frame.method)
    )
      return false
    const params = frame.params
    if (
      !params ||
      typeof params !== "object" ||
      Array.isArray(params) ||
      typeof params.sessionId !== "string" ||
      !params.sessionId ||
      (frame.method === "session/set_config_option"
        ? typeof params.configId !== "string" ||
          !params.configId ||
          typeof params.value !== "string"
        : typeof params.modeId !== "string" || !params.modeId)
    )
      throw new Error("invalid ACP configuration command")
    return frame.method === "session/set_config_option"
      ? params.configId === "mode" && params.value !== "read-only"
      : params.modeId !== "read-only"
  })
}
