/**
 * Where this client process is talking to.
 * Mirrors `nessa-server` `NESSA_STAGE` policy — not scattered one-off flags.
 */
export type Stage = "dev" | "alpha" | "ci" | "prod"

export const STAGES: readonly Stage[] = ["dev", "alpha", "ci", "prod"] as const

export const DEV_AUTH_TOKEN = "dev-token"

export function isStage(value: string): value is Stage {
  return (STAGES as readonly string[]).includes(value)
}

/** Only local `dev` may omit an explicit auth credential. */
export function stageAllowsDefaultAuth(stage: Stage): boolean {
  return stage === "dev"
}

export class StageConfigError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "StageConfigError"
  }
}
