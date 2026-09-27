import { gatewayOrigin, type Stage } from "./gateway-ports"

export type EnvSource = Readonly<Record<string, string | undefined>>
export type Environment = {
  readonly stage: Stage
  /**
   * Where the local gateway answers, as an origin with no trailing slash.
   *
   * Empty means this page's own origin: a development server proxies the
   * gateway onto it, so a browser preview reaches it without naming a port.
   * A packaged build has no proxy and talks to the gateway directly.
   */
  readonly gatewayBaseUrl: string
  /** Explicit gateway origin supplied by the build environment, if any. */
  readonly gatewayBaseUrlOverride?: string
  readonly conversation:
    | { readonly backend: "local" }
    | {
        readonly backend: "scenario"
        readonly scenario: "echo" | "offline"
      }
}

function gatewayBaseUrl(
  source: EnvSource,
  developmentBuild: boolean,
  stage: Stage,
): string {
  const configured = source.VITE_NESSA_GATEWAY_URL
  // A packaged build has no proxy, so it names the port its own stage listens
  // on — prod's 7420 for a shipped app, and the dev port for a dev stage.
  if (configured === undefined) return developmentBuild ? "" : gatewayOrigin(stage)
  if (configured === "") return ""
  let url: URL
  try {
    url = new URL(configured)
  } catch {
    throw new Error("VITE_NESSA_GATEWAY_URL must be an absolute http or https URL")
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("VITE_NESSA_GATEWAY_URL must be an absolute http or https URL")
  }
  return url.origin
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
  const gateway = gatewayBaseUrl(source, developmentBuild, stage)
  const gatewayBaseUrlOverride =
    source.VITE_NESSA_GATEWAY_URL === undefined || source.VITE_NESSA_GATEWAY_URL === ""
      ? undefined
      : gateway
  const backend = source.VITE_NESSA_CONVERSATION_BACKEND ?? "local"
  const scenario = source.VITE_NESSA_CONVERSATION_SCENARIO
  if (backend === "local") {
    if (scenario !== undefined)
      throw new Error("Conversation scenario requires scenario backend")
    return {
      stage,
      gatewayBaseUrl: gateway,
      gatewayBaseUrlOverride,
      conversation: { backend },
    }
  }
  if (backend !== "scenario") throw new Error("Unknown conversation backend")
  if (!developmentBuild || (stage !== "dev" && stage !== "ci")) {
    throw new Error("Scenario backend requires a development build in dev or ci stage")
  }
  if (scenario !== "echo" && scenario !== "offline") {
    throw new Error("VITE_NESSA_CONVERSATION_SCENARIO must be echo or offline")
  }
  return {
    stage,
    gatewayBaseUrl: gateway,
    gatewayBaseUrlOverride,
    conversation: { backend, scenario },
  }
}
