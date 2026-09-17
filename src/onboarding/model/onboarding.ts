/** First-run setup: which agent runs a conversation, and how far setup has got.
 *
 * This is the panel's own chrome state, not product state: it decides what the
 * window shows before a conversation exists. The choice is recorded here and
 * nothing else — connecting the selected agent to a live gateway is a separate
 * step that does not exist yet, so nothing in this module claims an agent is
 * ready to run.
 */

/** An agent Nessa can be set up against. */
export type AgentId = "claude" | "codex"

/** What the panel knows about one selectable agent. */
export interface AgentChoice {
  id: AgentId
  /** Product name, as a person would say it. */
  name: string
  /** One line on what picking it means. */
  summary: string
  /** False while no provider adapter can run this agent yet. */
  available: boolean
}

/** The agents offered at first run, in presentation order.
 *
 * Codex is listed and explicitly unavailable rather than hidden: the roadmap is
 * part of the choice, and an unavailable entry cannot be selected, so the panel
 * never offers a capability the runtime lacks.
 */
export const AGENT_CHOICES: readonly AgentChoice[] = Object.freeze([
  Object.freeze({
    id: "claude" as const,
    name: "Claude",
    summary: "Anthropic's agent, over the bundled Claude ACP runtime.",
    available: true,
  }),
  Object.freeze({
    id: "codex" as const,
    name: "Codex",
    summary: "OpenAI's agent. Not available yet.",
    available: false,
  }),
])

/** Ordered first-run steps. `done` means the panel shows the conversation. */
export type OnboardingStep = "welcome" | "agent" | "summon" | "done"

/** What the panel is showing during first run. */
export interface OnboardingState {
  step: OnboardingStep
  /** The agent picked so far, if any. Never an unavailable one. */
  agent?: AgentId
}

/** The state a panel with no completed setup starts from. */
export function beginOnboarding(): OnboardingState {
  return { step: "welcome" }
}

/** Look up a listed agent. Unknown identities have no choice to return. */
export function agentChoice(id: AgentId): AgentChoice | undefined {
  return AGENT_CHOICES.find((choice) => choice.id === id)
}

/** Move from the welcome step to the agent picker. Other steps do not move. */
export function startAgentChoice(state: OnboardingState): OnboardingState {
  return state.step === "welcome" ? { ...state, step: "agent" } : state
}

/** Record a selected agent. An unavailable or unknown agent is not selectable,
 * so the state is returned unchanged rather than storing a choice the runtime
 * cannot honour. */
export function chooseAgent(state: OnboardingState, id: AgentId): OnboardingState {
  if (state.step !== "agent") return state
  return agentChoice(id)?.available ? { ...state, agent: id } : state
}

/** Move from the picker to the summon step. Only a recorded agent moves on;
 * without one the picker stays on screen. */
export function confirmAgent(state: OnboardingState): OnboardingState {
  if (state.step !== "agent") return state
  return state.agent ? { ...state, step: "summon" } : state
}

/** Leave setup without finishing it, from any step. Nothing is recorded: an
 * agent chosen on the way out is not a completed setup, and the next run starts
 * over. */
export function dismissOnboarding(state: OnboardingState): OnboardingState {
  return state.step === "done" ? state : { step: "done" }
}

/** Finish setup from the summon step. */
export function completeOnboarding(state: OnboardingState): OnboardingState {
  return state.step === "summon" ? { ...state, step: "done" } : state
}

/** Whether the panel should show setup instead of the conversation. */
export function isOnboarding(state: OnboardingState): boolean {
  return state.step !== "done"
}
