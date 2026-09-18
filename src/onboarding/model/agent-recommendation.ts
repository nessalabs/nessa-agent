/** What first-run setup should suggest when somebody has no agent to pick.
 *
 * Setup lists every agent and lets the person choose. That works when they
 * already have one. Someone who has never installed a coding agent reaches the
 * picker, finds nothing selectable, and is told to go away and install
 * something before Nessa can do anything at all — which is the worst moment in
 * the whole product to hand somebody a homework assignment.
 *
 * Nessa can install an agent itself, so on that screen there is something
 * better to say. This module decides what.
 *
 * Deliberately not written around one agent's name. Which agents Nessa can
 * fetch is a fact the server owns — it pins the releases it has tested — and
 * naming one here would put that truth in two places that could disagree.
 */

import { AGENT_CHOICES, agentChoice, agentReadiness } from "./onboarding"
import type { AgentId, OnboardingState } from "./onboarding"

/**
 * What setup should offer.
 *
 * Three cases, because they are three different screens. `choose` is the
 * ordinary picker. `install` is the picker with an offer on it. `nothing` is
 * the picker with no offer, which is what an unanswered question has to look
 * like — see below.
 */
export type AgentRecommendation =
  /** At least one agent can start. The person picks; Nessa suggests nothing. */
  | { readonly kind: "choose" }
  /** No agent can start, and this is one Nessa can install. */
  | { readonly kind: "install"; readonly agent: AgentId }
  /** No agent can start and there is nothing to offer, or nobody answered. */
  | { readonly kind: "nothing" }

/**
 * Whether any listed agent is ready to start.
 *
 * Only `ready` counts. Every other state — needing a sign-in, missing, not
 * configured, unsupported, unanswered — is not an agent somebody can use right
 * now, and this deliberately does not enumerate them: a state added later is
 * not ready until somebody decides it is.
 */
function anythingReady(state: OnboardingState): boolean {
  return AGENT_CHOICES.some((choice) => agentReadiness(state, choice.id) === "ready")
}

/**
 * Whether Nessa could actually put this agent in front of somebody.
 *
 * Being installable is not enough. The server can pin a release for an agent
 * this build has no adapter for — that is exactly the state an agent passes
 * through while it is being added — and offering to download one would end with
 * a hundred megabytes on disk and a row in the picker that still cannot be
 * selected.
 */
function offerable(agent: AgentId): boolean {
  return agentChoice(agent)?.supported === true
}

/**
 * What to suggest, given what the runtimes said and what Nessa can install.
 *
 * `installable` is the list of agents Nessa can fetch, which is a fact about
 * this build rather than about the machine. Passing it in rather than naming an
 * agent here keeps that single source of truth on the side that owns it.
 *
 * Deliberately not offered on the strength of a missing answer: a report that
 * never arrived is not evidence that somebody has no agent, and offering a
 * large download to a person whose gateway was briefly unreachable would be
 * acting on a question nobody answered. That is the rule the rest of setup
 * follows, and it is why an unanswered ask reads as no recommendation rather
 * than as an empty machine.
 */
export function recommendAgent(
  state: OnboardingState,
  installable: readonly AgentId[],
): AgentRecommendation {
  // Nothing reported at all. Not an empty machine: an unasked question.
  if (!state.readiness) return { kind: "nothing" }
  if (anythingReady(state)) return { kind: "choose" }
  const offer = installable.find(offerable)
  return offer ? { kind: "install", agent: offer } : { kind: "nothing" }
}

/** Whether setup should show an install offer at all. */
export function offersInstall(recommendation: AgentRecommendation): boolean {
  return recommendation.kind === "install"
}
