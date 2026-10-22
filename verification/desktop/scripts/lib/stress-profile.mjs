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

/** Reject an incompatible exact fixture; do not fall back or rewrite the workload. */
export function requireStressCapabilities(profile, model) {
  if (model?.modelId !== profile.model)
    throw new Error(`stress fixture model differs from requested ${profile.model}`)
  const offered = model.approvalModes?.map((choice) => choice.id)
  const required = profile.scenario === "full" ? ["ask", "auto", "full"] : ["ask"]
  if (
    !offered ||
    offered.length !== required.length ||
    !required.every((id) => offered.includes(id))
  )
    throw new Error(
      `${profile.scenario} scenario is incompatible with ${profile.model}: advertised ${JSON.stringify(offered)}, required ${JSON.stringify(required)}`,
    )
  return { ...profile, advertisedModes: [...offered], requiredModes: required }
}

/** Retain the full recording prefix; a refused pick must never configure elevated access. */
export function elevatedAccessAfter(before, after) {
  if (!after.startsWith(before) || !after.endsWith("\n"))
    throw new Error("incomplete or replaced ACP recording")
  return after
    .slice(before.length)
    .split("\n")
    .filter(Boolean)
    .some((line) => {
      const { direction, frame } = JSON.parse(line)
      if (
        !["to-agent", "from-agent"].includes(direction) ||
        !frame ||
        frame.jsonrpc !== "2.0"
      )
        throw new Error("invalid ACP recording")
      return (
        direction === "to-agent" &&
        ((frame.method === "session/set_config_option" &&
          frame.params?.configId === "mode" &&
          frame.params.value !== "read-only") ||
          (frame.method === "session/set_mode" && frame.params?.modeId !== "read-only"))
      )
    })
}
