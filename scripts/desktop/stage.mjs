import { readFileSync } from "node:fs"

const gatewayPorts = JSON.parse(
  readFileSync(
    new URL("../../protocol/defaults/gateway-ports.json", import.meta.url),
    "utf8",
  ),
)

function explicitStage(environment, name) {
  if (!Object.hasOwn(environment, name)) return undefined
  const value = environment[name]
  if (typeof value !== "string" || value.length === 0 || value !== value.trim()) {
    throw new Error(`${name} must name a stage without surrounding whitespace`)
  }
  return value
}

function knownStage(stage) {
  if (!Object.hasOwn(gatewayPorts.stages, stage)) {
    throw new Error(
      `Unknown Nessa stage "${stage}". Choose one of: ${Object.keys(gatewayPorts.stages).join(", ")}`,
    )
  }
  return stage
}

/** Resolve one stage for the UI, host, and gateway before a desktop command starts. */
export function resolveDesktopStage({ environment, fallback, requested }) {
  const fromArgument = requested === undefined ? undefined : knownStage(requested)
  const host = explicitStage(environment, "NESSA_STAGE")
  const ui = explicitStage(environment, "VITE_NESSA_STAGE")
  const stated = [fromArgument, host, ui].filter((value) => value !== undefined)
  const stage = knownStage(stated[0] ?? fallback)
  for (const value of stated.slice(1)) {
    if (value !== stage) {
      throw new Error(
        `Desktop stage conflict: build stage "${stage}", NESSA_STAGE "${host ?? "unset"}", ` +
          `VITE_NESSA_STAGE "${ui ?? "unset"}". Set all explicit stage values to the same name.`,
      )
    }
  }
  return stage
}

/** Environment handed to Tauri and its before-build/dev frontend command. */
export function desktopStageEnvironment(environment, stage) {
  return {
    ...environment,
    NESSA_STAGE: stage,
    VITE_NESSA_STAGE: stage,
  }
}
