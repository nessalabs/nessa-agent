export type EnvSource = Readonly<Record<string, string | undefined>>
export type Environment = {
  readonly stage: "dev" | "ci" | "alpha" | "prod"
  readonly conversation:
    | { readonly backend: "local" }
    | {
        readonly backend: "scenario"
        readonly scenario: "echo" | "offline"
      }
}

/** Pure parser. Tests pass a map; only the Vite adapter reads import.meta.env. */
export function loadEnvironment(
  source: EnvSource,
  developmentBuild = false,
): Environment {
  const stage = source.VITE_NESSA_STAGE ?? (developmentBuild ? "dev" : "prod")
  if (stage !== "dev" && stage !== "ci" && stage !== "alpha" && stage !== "prod") {
    throw new Error("VITE_NESSA_STAGE must be dev, ci, alpha, or prod")
  }
  const backend = source.VITE_NESSA_CONVERSATION_BACKEND ?? "local"
  const scenario = source.VITE_NESSA_CONVERSATION_SCENARIO
  if (backend === "local") {
    if (scenario !== undefined)
      throw new Error("Conversation scenario requires scenario backend")
    return { stage, conversation: { backend } }
  }
  if (backend !== "scenario") throw new Error("Unknown conversation backend")
  if (!developmentBuild || (stage !== "dev" && stage !== "ci")) {
    throw new Error("Scenario backend requires a development build in dev or ci stage")
  }
  if (scenario !== "echo" && scenario !== "offline") {
    throw new Error("VITE_NESSA_CONVERSATION_SCENARIO must be echo or offline")
  }
  return { stage, conversation: { backend, scenario } }
}
