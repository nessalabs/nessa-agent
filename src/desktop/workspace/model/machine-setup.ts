import { agentChoice, type AgentId } from "../../../onboarding/model/onboarding"

/**
 * Getting a chat's cloud machine ready, as the gateway reports it
 * (`ConversationView.setup`): each step on the window's clock. The gateway
 * owns which step it is on; this module owns only the words for it and when
 * a finished one is worth a line.
 */
export type MachineSetupStepKind =
  "creating" | "waking" | "installing" | "connecting" | "starting"

export interface MachineSetupStep {
  readonly step: MachineSetupStepKind
  /** When it began, on the window's clock. */
  readonly startedAtMs: number
  /** When it ended, on the window's clock; absent while it runs. */
  readonly endedAtMs?: number
}

export interface MachineSetup {
  readonly state: "running" | "ready" | "failed"
  /** The message whose send this opening served. */
  readonly executionId?: string
  readonly steps: readonly MachineSetupStep[]
}

/**
 * The words for a step, as the live row says them. "Starting" names the
 * chat's agent when the source said which one it is.
 */
export function setupStepLabel(step: MachineSetupStepKind, agent?: string): string {
  switch (step) {
    case "creating":
      return "Setting up container"
    case "waking":
      return "Waking container"
    case "installing":
      return "Installing Nessa"
    case "connecting":
      return "Connecting"
    case "starting": {
      const name = agent === undefined ? undefined : agentChoice(agent as AgentId)?.name
      return name ? `Starting ${name}` : "Starting agent"
    }
  }
}

/** The opening under way, if any. */
export function runningSetup(
  setups: readonly MachineSetup[] | undefined,
): MachineSetup | undefined {
  return setups?.find((setup) => setup.state === "running" && setup.steps.length > 0)
}

/**
 * A machine ready this quickly says nothing after the reply: the wait was
 * not one a person noticed.
 */
export const SETUP_WORTH_A_LINE_MS = 2000

/** How long a finished opening took, from its first step to its last. */
export function setupTookMs(setup: MachineSetup): number {
  const first = setup.steps[0]
  const last = setup.steps[setup.steps.length - 1]
  if (!first || !last) return 0
  return Math.max(0, (last.endedAtMs ?? last.startedAtMs) - first.startedAtMs)
}

/** The folded line above a reply whose send waited on its machine. */
export function setupReadyLabel(setup: MachineSetup): string {
  return `Container ready in ${Math.max(1, Math.round(setupTookMs(setup) / 1000))}s`
}

/**
 * Whether the machine opening that `executionId`'s send waited on failed: one
 * that names it, or, for a chat the gateway has taken no message of, one
 * that names none (made as the chat was created, which its first send
 * waited on; the gateway lists no other opening that names no message).
 */
export function setupFailedFor(
  setups: readonly MachineSetup[] | undefined,
  executionId: string,
  answered = true,
): boolean {
  return (
    setups?.some(
      (setup) =>
        setup.state === "failed" &&
        (setup.executionId === executionId ||
          (!answered && setup.executionId === undefined)),
    ) ?? false
  )
}
