/**
 * Where this client process is talking to.
 * Mirrors `nessa-server` `NESSA_STAGE` policy — not scattered one-off flags.
 */
export const Stage = { Dev: "dev", Alpha: "alpha", Ci: "ci", Prod: "prod" } as const

export type Stage = "dev" | "alpha" | "ci" | "prod"

export const STAGES: readonly Stage[] = ["dev", "alpha", "ci", "prod"] as const

export function isStage(value: string): value is Stage {
  return (STAGES as readonly string[]).includes(value)
}

/** Only dev may omit an explicit loopback URL. */
export function stageAllowsDefaultUrl(stage: Stage): boolean {
  return stage === "dev"
}

export class StageConfigError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "StageConfigError"
  }
}
