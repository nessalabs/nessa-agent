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

/**
 * What stands between an agent and running, as its runtime reports it.
 *
 * Only `ready` may be chosen. The rest are shown rather than hidden, because an
 * agent missing from the list is a question — "where is Claude?" — and an agent
 * listed with a reason is an answer. That only holds while the reasons stay
 * distinct: "Nessa has no adapter for this" and "nobody answered" are different
 * facts and a person can act on only one of them.
 */
export type AgentReadiness =
  /** Installed, signed in, and able to start. */
  | "ready"
  /** Installed, but nothing here is signed in to it. */
  | "needs-authentication"
  /** Nothing to sign in to: the agent's own runtime is not on this machine. */
  | "not-installed"
  /** Nessa has no adapter for this agent at all. The one reason that will not
   * change by doing anything on this machine. */
  | "not-supported"
  /** Nobody has answered for it — it has not been asked yet, the gateway could
   * not be reached, or it said something this build cannot read. Never a
   * negative answer about the agent itself. */
  | "unknown"

/** Why nothing is known about any agent, when the ask itself failed. */
export type AgentReadinessFailure =
  /** Nothing usable came back: the gateway is not running, or refused. */
  | "unreachable"
  /** Something came back in a shape this build cannot read. */
  | "unreadable"

/** What each agent's runtime reports. Absent for an agent not yet asked about. */
export type AgentReadinessReport = Readonly<Partial<Record<AgentId, AgentReadiness>>>

/** What the panel knows about one listed agent. */
export interface AgentChoice {
  id: AgentId
  /** Product name, as a person would say it. */
  name: string
  /** False while Nessa has no adapter for this agent at all, whatever is
   * installed. That is a different thing from an adapter that cannot run yet,
   * and it is the only one that will not change by signing in. */
  supported: boolean
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
    supported: true,
  }),
  Object.freeze({
    id: "codex" as const,
    name: "Codex",
    supported: false,
  }),
])

/** Ordered first-run steps. `done` means the panel shows the conversation.
 *
 * There is no step after the shortcut lesson. Finishing it is already the good
 * news, and a screen whose only job is to say so again is a screen between
 * someone and the thing they came for. */
export type OnboardingStep = "welcome" | "agent" | "summon" | "done"

/** What the panel is showing during first run. */
export interface OnboardingState {
  step: OnboardingStep
  /** The agent picked so far, if any. Never an unavailable one. */
  agent?: AgentId
  /**
   * What the panel is doing now, as the host reported it — not a count of
   * presses. The two can disagree: the panel may already be on screen when the
   * step is reached, and then the first press hides it. A model that counted
   * would say "shown" while the sound, the panel and the copy all said
   * otherwise. Undefined means nothing has been reported yet.
   */
  summon?: "shown" | "hidden"
  /**
   * Whether the shortcut has been seen to put the panel away.
   *
   * That is the half of the toggle the step exists to teach: someone who only
   * ever sees it summon has been taught how to put Nessa on screen and not how
   * to get rid of it. Once taught it stays taught — a later press must not take
   * the way on back off the screen.
   */
  summonTaught?: boolean
  /**
   * What each agent's runtime reported when it was asked. Absent until it has
   * been: nothing is choosable before the answer arrives, because offering an
   * agent and then finding out it cannot run is the failure this exists to
   * avoid.
   */
  readiness?: AgentReadinessReport
  /**
   * Why the ask failed, when it did. Kept apart from the report so "we could
   * not find out" is never shown as a fact about an agent.
   */
  readinessFailure?: AgentReadinessFailure
}

/** The state a panel with no completed setup starts from. */
export function beginOnboarding(): OnboardingState {
  return { step: "welcome" }
}

/** Look up a listed agent. Unknown identities have no choice to return. */
export function agentChoice(id: AgentId): AgentChoice | undefined {
  return AGENT_CHOICES.find((choice) => choice.id === id)
}

/**
 * Apply a newly arrived answer, and drop a selection it has just invalidated.
 *
 * The runtimes can be asked more than once — the picker offers it, and reaching
 * the picker does it — so an answer can arrive that contradicts the one a choice
 * was made against. While the picker is still up, a choice the newest answer
 * says cannot run is not kept: leaving it ticked, with Continue still lit, is
 * the panel promising an agent it has just been told is not there.
 *
 * Past the picker the choice is not silently unmade. Setup has moved on, and
 * quietly emptying a decision behind someone on a step that says nothing about
 * agents would be a worse lie than the stale tick.
 */
function withAnswer(
  state: OnboardingState,
  readiness: AgentReadinessReport | undefined,
  readinessFailure: AgentReadinessFailure | undefined,
): OnboardingState {
  const answered: OnboardingState = { ...state, readiness, readinessFailure }
  if (answered.step !== "agent" || !answered.agent) return answered
  return isChoosable(answered, answered.agent)
    ? answered
    : { ...answered, agent: undefined }
}

/** Record what the runtimes reported. A report also clears any earlier failure:
 * the question has now been answered. */
export function recordReadiness(
  state: OnboardingState,
  readiness: AgentReadinessReport,
): OnboardingState {
  return withAnswer(state, readiness, undefined)
}

/** Record that nobody answered, and why. The report is dropped rather than
 * kept: a stale "ready" from an earlier ask is not evidence that an agent whose
 * gateway has since gone away can still start. */
export function recordReadinessFailure(
  state: OnboardingState,
  reason: AgentReadinessFailure,
): OnboardingState {
  return withAnswer(state, undefined, reason)
}

/** What an agent's runtime reported.
 *
 * `not-supported` is a real negative answer and belongs to the listing, not to
 * any runtime. `unknown` is the absence of an answer — not asked yet, or asked
 * and not answered — and an agent is never offered on the strength of it. */
export function agentReadiness(state: OnboardingState, id: AgentId): AgentReadiness {
  if (!agentChoice(id)?.supported) return "not-supported"
  return state.readiness?.[id] ?? "unknown"
}

/** Whether an agent may be picked: its adapter exists, it is installed, and
 * something here is signed in to it. */
export function isChoosable(state: OnboardingState, id: AgentId): boolean {
  return agentReadiness(state, id) === "ready"
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
  return isChoosable(state, id) ? { ...state, agent: id } : state
}

/** Move from the picker to the summon step. Only a recorded agent moves on;
 * without one the picker stays on screen. */
export function confirmAgent(state: OnboardingState): OnboardingState {
  if (state.step !== "agent") return state
  return state.agent ? { ...state, step: "summon" } : state
}

/**
 * Record what the summon shortcut just did, as the host reports it.
 *
 * `showing` is the panel's actual visibility after the press, not a count of
 * presses. Counting was wrong in a way nobody could recover from: a panel
 * already on screen when the step is reached is *hidden* by the first press,
 * and the copy, the cue and the keycaps would then each be describing a
 * different panel.
 *
 * Seeing it put the panel away is what the step is for, so that is what is
 * remembered; a later press moves `summon` but cannot un-teach it.
 *
 * Presses outside that step are somebody using their shortcut, not setup, and
 * change nothing.
 */
export function pressSummon(state: OnboardingState, showing: boolean): OnboardingState {
  if (state.step !== "summon") return state
  const summon = showing ? "shown" : "hidden"
  const summonTaught = state.summonTaught === true || !showing
  if (state.summon === summon && state.summonTaught === summonTaught) return state
  return { ...state, summon, summonTaught }
}

/** Leave setup without finishing it, from any step. Nothing is recorded: an
 * agent chosen on the way out is not a completed setup, and the next run starts
 * over. */
export function dismissOnboarding(state: OnboardingState): OnboardingState {
  return state.step === "done" ? state : { step: "done" }
}

/**
 * Finish setup from the last step, keeping the agent that was chosen.
 *
 * The shortcut lesson is what the step is for; it is not what setup is for.
 * Requiring it to finish made setup impossible to complete for a configuration
 * that registers no summon accelerator, and for anyone who cannot hold a chord
 * — with no way out but abandoning setup, which records nothing and starts over
 * next launch. Whether the lesson landed is in `summonTaught`, and the surface
 * decides what to offer on the strength of it; refusing to finish is not that
 * decision. The lesson does not travel into the finished state — it was about
 * the step, not the setup.
 */
export function completeOnboarding(state: OnboardingState): OnboardingState {
  if (state.step !== "summon") return state
  return { step: "done", agent: state.agent }
}

/** Whether the panel should show setup instead of the conversation. */
export function isOnboarding(state: OnboardingState): boolean {
  return state.step !== "done"
}
